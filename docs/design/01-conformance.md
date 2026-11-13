# RadioPost: the conformance tester

| | |
|---|---|
| **Status** | Draft |
| **Version** | 0.4.2 |
| **Updated** | 2026-11-13 |
| **Editor** | KK4ODA |
| **Licence** | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| **Follows** | [The detailed design](00-detailed-design.md), §7 |

An outline of the program that tells an implementer whether their program does what the
specifications require. It was Phase 0's last part; Phase 1 built its first half and Phase 2 its
gateway and post office tests (§6), as `core/tester` (`radiopost-tester`).

---

## 1. What it is for

* It says whether a program does what the specifications require in a role (RP-0 §3), before the
  program goes on the air. When it does not, the tester names the section and shows the bytes.
* It covers what the vectors cannot: a running program, over time, when things go wrong. The vectors
  check encodings and verdicts; each implementation runs them in its own tests, as Aether's core
  runs its model's.
* Passing is not Candidate. RP-0 §8.1 asks for two independent implementations that complete
  sessions with each other. The tester finds problems before that; it does not replace it.
* Out of scope: radio, real modems' timing, and whether a band or a time is allowed. Those are the
  modem's and the operator's (RP-4 §8).

---

## 2. How it works

### 2.1 It plays the other side

| Program under test | The tester plays | Over |
|---|---|---|
| Client | A gateway, with its post office inside; or another client, station to station | RPMI, through the test adapter (§2.2) |
| Gateway | Clients and, for RP-7, partner gateways on the air side; the post office on the Internet side | The test adapter; HTTPS to the test post office |
| Post office | Gateways and clients; a mail server that sends to its door and takes what it sends | HTTPS (RP-5), SMTP (RP-6), a test DNS zone |
| Modem adapter | The host, and a second instance of the adapter as the far station | RPMI; the adapter's own link (TCP, or a modem's simulated channel) |

### 2.2 The test adapter

The program reaches the air through RP-4 as usual, but the adapter it talks to is the tester's: an
RPMI adapter on the local machine (RP-4 §2) with nothing on the air behind it. A call is answered by
the tester, and `received` carries the tester's bytes. Because the tester is the modem, it can:

* break the link after any byte, or before a transmission (RP-3 §7);
* hold bytes back, to test timeouts (RP-3 §8);
* report the channel busy, refuse a call, or fail a connect with each `failed` code (RP-4 §5, §7);
* deliver bytes split anywhere, since `received` keeps no boundaries (RP-4 §5).

Joining two hosts instead of one host and the tester, the same program is the loopback adapter of
the detailed design (§7 there), for tests with no radio.

### 2.3 Conformance mode

To be tested, a program offers a mode that:

* trusts the test post office keys of `vectors/keys.json` and the tester's revocation lists. Their
  private halves are published: they are never real keys;
* uses the test adapter and the test post office URLs it is given, and never looks up the test
  domains in Internet DNS (`radiopost.org` and `hampost.org` stand for test post offices here);
* refuses every adapter but the test one, so test keys never reach the air.

It needs nothing else. The program keeps its own clock and random numbers and makes its own keys.

A gateway in conformance mode asks only the tester's DNS resolver (DNS over HTTPS, serving the
test zone) and reaches the test post office at the URL it is given: the test post office serves
plain HTTP on the loopback interface, which RP-1 §7.1 has a reader ignore in DNS. Its driver file
also names `post_office`, `block`, `credential` (a credential the test post office issued for its
key) and `serve`; `radiopost-gateway`'s is `core/tester/drivers/radiopost-gateway.json`. The test
post office is the core's own post office on the published test key, whose every exchange the
tests read.

A post office is started by the tester in a fresh directory for each test, from its driver file
(`init`, `know`, `dns`, `code`, `serve`, and `door`, the URL of its door); the tester chooses its
addresses and runs the mail server's two sides: it calls the door in Stalwart's shape, its verdicts
(DMARC, the domains DKIM passes for) in the mail server's `serverHeaders`, and takes what the post
office submits on a stand-in SMTP port. `radiopost-po`'s is
`core/tester/drivers/radiopost-po.json`.

An adapter is tested as it runs: the tester connects to it as its host (`--at`), and for the
tests that need the air, to a second instance joined to it (`--far`) as the far station's host,
named in `connect` as `--reach` says. It allows a radio's timing for anything that crosses.

A **driver file** tells the tester how to run the program: a JSON object naming each command it
needs (`conformance`, `key`, `init`, `trust`, `compose`, `call`, `call_station`, `listen`, `inbox`,
`outbox`) as a list of words, with `{bin}`, `{home}`, `{adapter}` and the test's values put in.
Each test runs the program in a fresh home, which it puts in conformance mode first. The
`radiopost` client's is `core/tester/drivers/radiopost.json`; its conformance mode is
`radiopost conformance --adapter HOST:PORT`.
The tester issues its credentials as the test post office, through enrollment (RP-5 §6.1) or
gateway registration (RP-5 §6.4), and signs fresh statements on its own clock. So the checks are
semantic: each PDU, line and request is decoded and checked field by field against the
specification, its signatures are verified, and the program's next move is compared with what the
specification allows.

### 2.4 Replay, if the program can

A program that also takes a fixed clock and fixed random draws (session IDs, nonces, challenges)
can be replayed against the session files byte for byte (`session-01.json` to `session-05.json`).
That is the strictest check and the easiest to debug, but it needs hooks a deployed program does not,
so it is optional.

---

## 3. What it checks

Each test has an ID, the sections it checks, the vectors it builds on, and a grade: **MUST** (a
failure fails the role) or **SHOULD** (reported). All the tests below are MUST unless marked. This is
the outline; each phase writes its tests in full (§6).

### 3.1 A client

RP-0 §3: RP-1 §4, §5, §6, RP-2, RP-3 client side.

| ID | Checks | Builds on |
|---|---|---|
| C-01 | An ordinary session: `HELLO` signed in time form, offers, `MSG` pieces, `RECEIPT`, `REQUEST`, `DONE`, and the hang-up (RP-3 §3, §5, §8). | `session-01` |
| C-02 | A challenge: `WELCOME` `status` `sign` answered with `AUTH` over it (RP-3 §5.3, RP-1 §6.1). | `statements` |
| C-03 | The credential by hash, then the full credential when asked (RP-3 §5.1, §5.8). | `pdus` |
| C-04 | Uploads and downloads broken at chosen bytes and resumed, through the same gateway and another; the same air form kept (RP-3 §7). | `session-02` |
| C-05 | Refusals: `decline` codes, `ERROR`, failed messages; nothing sent that was not accepted (RP-3 §5.2, §5.4, §5.9). | `api-exchanges` |
| C-06 | Bad frames from the gateway (an unknown type, not CBOR) answered `ERROR` `malformed`, and one over the program's `max_pdu` `ERROR` `too-large` (RP-3 §2, §4). | `pdus` `invalid` |
| C-07 | Station to station, calling: the station called's proof checked before anything is taken; a bad one answered `ERROR` and the call ended (RP-3 §10, RP-1 §6.5). | `session-03`, `statements` |
| C-08 | Station to station, called: only mail for its own account taken, its own proof given (RP-3 §10). | `session-03` |
| C-09 | A cut-off gateway: mail kept as queued and offered again until an online gateway confirms it (RP-7 §3.6). | `queued` |
| C-10 | Local mail: listed with serial 0, asked for by content ID, taken without a serial (RP-3 §5.2, §5.6, RP-7 §4.4). | `session-05` |
| C-11 | 300 seconds of nothing end the session, and the caller hangs up (RP-3 §8). | — |
| C-12 | Station-to-station sessions reported over the Internet (RP-5 §6.6). | `direct` |

### 3.2 A gateway

RP-0 §3: RP-1, RP-2 §2, §3, §4, §5, RP-3 gateway side, RP-5 §5 as the caller.

| ID | Checks | Builds on |
|---|---|---|
| G-01 | `session-01` at an online gateway: the post office asked while the link turns around, `WELCOME` within a second (SHOULD), the report and the session closed (RP-3 §9, RP-5 §5.2, §5.7). | `session-01`, `api-exchanges` |
| G-02 | Credentials and statements refused in RP-1 §5's and §6.3's order, each with its code: expired, revoked, an unknown post office, a bad signature, a skewed clock, a replayed nonce (RP-3 §5.9). | `credentials`, `statements` |
| G-03 | Offline: users checked with the keys and revocation lists it holds, copies served, the queue kept, everything reported when the Internet returns (RP-1 §7.3, RP-5 §5.6, §5.8). | `api-exchanges` |
| G-04 | Resumes across gateways (RP-3 §7). | `session-02` |
| G-05 | Third-party mail held back from a station abroad, with a `NOTICE` (RP-6 §5.1). | `third-party` |
| G-06 | Local delivery while cut off (RP-7 §4.4). | `session-05`, `local-delivery` |
| G-07 | A misbehaving caller: a PDU over the limit, an unknown type, a `MSG` for an offer not accepted, a caller that never hangs up (RP-3 §2, §5.4, §8). | `pdus` |
| G-08 | Renewal by radio (RP-3 §5.8, RP-5 §6.2). | `api-account` |
| G-09 | Which post offices it trusts: by DNS, one hop of lists, blocked ones unknown (RP-1 §7.3). | `dns` |

### 3.3 A forwarding gateway

RP-7, which a gateway MAY take.

| ID | Checks | Builds on |
|---|---|---|
| F-01 | It calls a partner only inside its windows, at most three tries a minute apart, not on a busy channel (RP-7 §5). | `forwarding` |
| F-02 | Records sent as `PROOF` and checked: the path and its limits, the credential by hash once held (RP-7 §3.2, §3.3). | `session-04`, `forwarding` |
| F-03 | Placements checked in order, with their codes (RP-7 §4.3). | `forwarding` |
| F-04 | Records kept for a partner that reaches the post offices (RP-7 §6.2). | `forwarding` |
| F-05 | Windows published and used only inside its country's segments (RP-7 §5.1). | `forwarding` |
| F-06 | Mail offered again answered `already-have`, or taken (RP-7 §3.6). | `queued` |

### 3.4 A post office

RP-0 §3: RP-1 as issuer, RP-2, RP-5 as server, RP-6.

| ID | Checks | Builds on |
|---|---|---|
| P-01 | Its public documents: DNS records, the info document, revocations, lists of post offices, gateways and routes, as JWS (RP-1 §7, RP-5 §4). | `dns`, `jws` |
| P-02 | Login and tokens; `401` against `403` (RP-5 §2, §3). | `api-exchanges` |
| P-03 | Enrollment and renewal (RP-5 §6.1, §6.2). | `api-exchanges`, `api-account` |
| P-04 | Sessions: the checks in order, messages taken and refused, pickups (RP-5 §5.2, §5.4, §5.5); a waiting message served with compression 0 or 1, its listed size that air form's (RP-5 §5.3). | `api-exchanges` |
| P-05 | Reports, with paths and local pickups (RP-5 §5.6). | `api-exchanges`, `forwarding`, `local-delivery` |
| P-06 | Copies placed ahead, forward items, drops (RP-5 §5.8, RP-7 §4.1). | `forwarding` |
| P-07 | Station-to-station reports (RP-5 §6.6). | `direct` |
| P-08 | The SMTP door: each refusal in order, the accept list (RP-6 §2.1, §2.2). | `email-door`, `api-account` |
| P-09 | Email to message and back, `Radio-Path`, bounces, the part between post offices (RP-6 §2.4, §3, §4). | `email-in`, `email-out` |
| P-10 | Third-party mail flagged (RP-6 §5.1, RP-5 §5.2). | `third-party`, `api-exchanges` |

### 3.5 A modem adapter

RP-0 §3: RP-4, the adapter's side.

| ID | Checks | Builds on |
|---|---|---|
| A-01 | The lines it reads and the ones it answers with an error (RP-4 §2). | `rpmi-lines` |
| A-02 | Opening, requests, events and errors in conversation, and a host that leaves with its link up (RP-4 §1.1, §3, §4, §5, §7). | `rpmi-conversations` |
| A-03 | A second host: turned away `busy` by an adapter that is not shared; on a shared one, its own callsigns, another's `claimed`, its own `status` (RP-4 §1.1, §3). Calls to each host of a shared modem come in Phase 5. | `rpmi-conversations` |
| A-04 | `session-01` carried between two instances, every byte intact (RP-4 §4, §5). | `rpmi-conversations` |
| A-05 | A transmission the rules forbid reported as `refused`, when the modem checks the rules (RP-4 §8). | — |

---

## 4. What it reports

* Each test passes, fails, or is skipped (the role does not include it, or it is a MAY the program
  says it does not take), with its sections.
* A failure carries the transcript in the vectors' own shapes (frames in hex with their diagnostic
  notation, RPMI lines, HTTP exchanges, emails) and the first point where the program left what the
  specification allows, in one sentence. A failing run reads like a vector, so it can become one.
* A summary in JSON, and an exit code: 0 when every MUST test of the roles asked for passed.
* A **conformance statement**: the program and its version, its roles, the specification versions,
  the tester's version, the date and the results. An implementer publishes it when a specification
  is proposed for Candidate (RP-0 §8.1).

---

## 5. How it is built

* A Rust program in `core/` (D27, D28). It uses the core library's encodings, which the vectors
  check, but has its own scripts for the other side: logic shared with the program under test
  would let a shared bug pass unseen.
* The scripts read the conversations from `vectors/` when they run, so the tester and the vectors
  cannot drift apart. A test's verdicts come from the specifications, never from what the reference
  programs do.
* It is tested as `make.py` is: each test is shown to fail on a planted fault, in a copy of a
  reference program built with that fault.
* CI runs it against the reference programs on every push, beside the vector checks.

---

## 6. Order of work

| Phase | The tester gains | Roles |
|---|---|---|
| 1 | The test adapter (and with it the loopback adapter); conformance mode; C-01 to C-08 and C-11; A-01 to A-04 against the TCP adapter. | Client, modem adapter |
| 2 | The test post office; G-01, G-02, G-04, G-07, G-08, G-09; P-01 to P-04 and P-07 to P-10; C-12 (built: G-09 checks DNS online, the one hop of lists being for a gateway offline, Phase 3; P-08 leaves out the case of the vectors' own `max_email_bytes`). | Gateway, online; post office |
| 3 | C-09, C-10; G-03, G-05, G-06; P-05, P-06. | Offline |
| 4 | F-01 to F-06. | Forwarding gateway |
| 5 | A-05, and A-01 to A-04 for each new adapter. | More adapters |

---

## 7. Not yet settled

* A public test service: a tester on the Internet, and a test gateway on the air for over-the-air
  trials, with the keys it would use.
* Tables for countries other than the United States (RP-6 §5.1, RP-7 §5.1): the tester checks US
  gateways first.

---

## Change log

| Version | Date | Change |
|---|---|---|
| 0.1.0 | 2026-11-09 | First outline: what the tester is for, how it plays the other side through its own adapter, conformance mode, the tests by role, what it reports, how it is built and tested, and the order of work. |
| 0.2.0 | 2026-11-09 | Phase 1's client tests built (`core/tester`): how a program is driven (a driver file, §2.3), which settles how conformance mode is turned on (§7). C-06 asks `too-large` for a frame over `max_pdu`, as RP-3 0.10.0 now says. |
| 0.3.0 | 2026-11-09 | Phase 1's adapter tests built: how an adapter is reached (§2.3); A-02 also checks a host that leaves (RP-4 0.5.0), and A-03 says what it covers. |
| 0.4.0 | 2026-11-10 | Phase 2's tests built: a gateway's and a post office's conformance mode and driver files (§2.3), the test post office, G-01, G-02, G-04, G-07 to G-09, P-01 to P-04, P-07 to P-10 and C-12 (§6). Writing them found a gateway that replayed a caller's last transmission while the link was quiet, and a post office that used an enrollment code at a refused attempt (RP-5 0.12.1). |
| 0.4.1 | 2026-11-10 | §2.3: the tester calls a post office's door in Stalwart's shape, the mail server's verdicts in `serverHeaders`, since a door no longer reads them from the message's own headers (RP-6 0.5.0). |
| 0.4.2 | 2026-11-13 | P-04 checks the air form a waiting message is served as (compression 0 or 1, RP-5 0.18.0 §5.3) and that the size listed is its own, not the uncompressed form's, which it had assumed. |
