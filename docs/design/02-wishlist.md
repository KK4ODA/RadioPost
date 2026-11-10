# RadioPost: the wishlist

| | |
|---|---|
| **Status** | Draft |
| **Version** | 0.1.0 |
| **Updated** | 2026-11-10 |
| **Editor** | KK4ODA |
| **Licence** | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| **Follows** | [The detailed design](00-detailed-design.md), §9 and §10 |

What the author and users ask the client, the gateway and the programs around them to do, in one
list: grouped, given a proposed priority, and checked against the specifications.

RadioPost is a common protocol that other people will write their own clients, gateways and
applications for. A feature that works only in our programs, or that needs a protocol change other
implementers cannot ignore, costs everyone. So every wish says what it would ask of the
specifications before anyone decides to do it.

The list starts with what the [concept](../concept/00-concept.md) and the detailed design promise
and have not built, marked as *the plan*, so that new wishes land in a list that already knows the
plan. Nothing is built because it is listed here, and no specification changes because of this
list: a wish taken up goes through the specifications' own process (RP-0 §8.4).

---

## 1. How to read it

Each wish has a number (W1, W2, …), never reused, which it keeps if it moves to another group. A
wish that repeats one already listed is merged into it, and its count rises. Under its name, one or
two sentences say what the user gets, and a table says the rest:

| Row | Holds |
|---|---|
| Asked | *The plan* (where the concept, the design or a specification promises it); *found* (seen while reading the documents and the programs, asked by no one yet); the author; users by callsign or name. Then how many people asked. |
| Program | Client, gateway, post office, adapter, or *on top* (an application carried in messages), or tools. |
| Compatibility | The verdict below, with the sections it rests on. |
| Status | Built, partly built, specified (written in a specification, not built), planned (named in the concept or the design, with its phase or decision), or new. |
| Depends on | Other wishes, or phases of the order of work (design §9). |
| Size | Small, medium or large, with why. |
| Priority | The proposed priority and its reason. The author's own call is marked **decided**, with the date; a reshuffle does not reopen it. |
| Notes | Where needed: how two wishes pull against each other, a rule to watch, a gap in a specification. |

**Compatibility.** RP-0 §8.2 sets the kinds of change. While the specifications are Drafts every
change to a requirement raises only their MINOR number, so a verdict says which kind the change is
by nature: whether a program written to today's text keeps working.

| Verdict | Means |
|---|---|
| **Fits today** | No protocol change. The sections it relies on are named. |
| **Minor addition** | An addition other implementations can ignore: a new optional key, endpoint, request or code (RP-0 §8.2, MINOR). The specification and what is added are named. |
| **Major change** | A change other implementations cannot ignore (RP-0 §8.2, MAJOR). What breaks is named. |
| **On top** | An application carried in ordinary messages, with no change below it. |
| **Conflicts** | With the amateur rules (nothing encrypted on the air, third-party traffic, unattended operation), with a decision in the design or by the author, or with another wish. What it conflicts with is named. |

**Size.** Small: days, one program, no protocol. Medium: weeks, or several programs, or a minor
addition with its test vectors. Large: a phase's worth, or a protocol change through several
programs.

**Priority.**

| | Means |
|---|---|
| **P1** | Now: needed for the pilot (Phase 2 on the air with users), or small and asked for by many. |
| **P2** | Next: Phase 3's work, or wanted by many and fits today. |
| **P3** | Later: Phases 4 and 5, or needs an addition to a specification and few have asked. |
| **P4** | Someday: large, needs a change others cannot ignore, or one person's wish against the plan. |

A proposal weighs how many asked, whether the pilot needs it, whether it fits today, what it
unblocks, and its size: a wish many users want that fits today ranks above one person's wish that
needs a major version.

---

## 2. Summary

| # | Wish | Group | Program | Asked | Priority | Compatibility | Status |
|---|---|---|---|---|---|---|---|
| W1 | Reply and forward | Mail | client | plan | P1 | fits today | planned |
| W2 | An address book and groups | Mail | client | plan | P2 | fits today | planned |
| W3 | Photos made small, and the time on the air shown | Mail | client | plan | P2 | fits today | planned |
| W4 | The full text of a trimmed message | Mail | client, gateway, post office | plan | P3 | minor: RP-3, RP-5, RP-6 | new (gap G1) |
| W5 | Mail kept as `.eml` files | Mail | client | plan | P3 | fits today | planned |
| W6 | Where each message is | Mail | client, post office | plan | P2 | fits today; minor (RP-5) for *delivered* | partly built |
| W7 | Several accounts in one client | Accounts | client | plan | P2 | fits today, a call each | planned |
| W8 | The credential renewed by radio | Accounts | client | plan | P1 | fits today | partly built |
| W9 | Usual gateways set in the panel | Accounts | client | plan | P2 | fits today | partly built |
| W10 | Enrollment with an LoTW certificate | Accounts | post office | plan | P2 | fits today | partly built |
| W11 | A trustee manages a role mailbox | Accounts | post office | plan | P3 | fits today | partly built |
| W12 | Email-only members | Accounts | post office | plan | P2 | fits today | waits on W36 |
| W13 | A callsign alias address | Accounts | post office | plan | P3 | fits today | planned |
| W14 | Choosing a gateway tunes the radio | Radio | client, adapter | plan | P2 | minor: RP-4 | planned |
| W15 | Keep listening for stations | Radio | client | found | P2 | fits today | new |
| W16 | Scheduled connections | Radio | client | plan | P3 | fits today | planned |
| W17 | Gateways ranked by propagation and measurements | Radio | client, gateway, post office | plan | P3 | fits today; minor (RP-5) for measurements | planned |
| W18 | Mail coming down compressed | Radio | post office | found | P1 | fits today | new |
| W19 | Shorter short messages: a dictionary | Radio | all | plan | P3 | minor; major if made a requirement | planned (D17) |
| W20 | A VARA HF and FM adapter | Radio | adapter | plan | P3 | fits today | planned, Phase 5 |
| W21 | An ARDOP adapter | Radio | adapter | plan | P3 | fits today | planned, Phase 5 |
| W22 | An AX.25 packet adapter | Radio | adapter | plan | P3 | fits today | planned, Phase 5 |
| W23 | A phone version | Radio | client | plan | P3 | fits today; minor (RP-4) for RPMI across devices | planned, Phase 5 |
| W24 | Block a caller by address or callsign | Gateway | gateway | plan | P2 | fits today | partly built |
| W25 | The gateway renews its own credential | Gateway | gateway | plan | P2 | fits today | partly built |
| W26 | One gateway, several radios | Gateway | gateway | plan | P3 | fits today | planned |
| W27 | Copies placed ahead | Offline | post office, gateway | plan | P2, order decided | fits today | specified, Phase 3 |
| W28 | The gateway's queue while offline, and its reports | Offline | gateway, post office | plan | P2, order decided | fits today | specified, Phase 3 |
| W29 | The sender's client follows queued mail | Offline | client | plan | P2 | fits today | specified, Phase 3 |
| W30 | Forwarding by radio | Offline | gateway, post office | plan | P3, order decided | fits today | specified, Phase 4 |
| W31 | Local delivery at a cut-off gateway | Offline | gateway, client | plan | P3 | fits today | specified, Phase 4 |
| W32 | Forms | Forms | client, post office | plan | P3 | minor: a new spec, RP-2, RP-6 (or on top) | planned, Phase 5 |
| W33 | Position reports | Forms | client, on top | plan | P3 | on top; minor as broadcasts | planned |
| W34 | Bulletins | Forms | on top, client | plan | P3 | on top; minor as broadcasts | planned |
| W35 | Services that answer by mail | Forms | on top | plan | P3 | on top | planned |
| W36 | One mailbox, by radio and by mail program | Post office | post office | plan | P2 | fits today | partly built |
| W37 | The accept list on a web page | Post office | post office | plan | P3 | fits today | planned |
| W38 | Backups and a backup mail server | Post office | post office | plan | P1 | fits today | planned |
| W39 | A domain served by several post offices | Post office | post office, all | plan | P4 | major: RP-1, RP-3 | a later option |
| W40 | The gateway checks the band rules itself | Rules | gateway | plan | P3 | fits today; minor (RP-4) to read the dial | partly built |
| W41 | Rules tables for other countries | Rules | gateway, specs | plan | P3 | minor: RP-6, RP-7 | planned |
| W42 | A monitor that decodes sessions off the air | Rules | on top | plan | P2 | fits today | planned |
| W43 | A public test service | Implementers | tools | plan | P3 | fits today | not settled |

---

## 3. Composing and reading mail

### W1. Reply and forward

Answer a message and keep its thread, at the other end too. Pass a message on to someone else.

| | |
|---|---|
| Asked | The plan: concept §10, first version. 0 people. |
| Program | Client. |
| Compatibility | **Fits today.** A reply sets `reply_to` to the content ID of the message it answers (RP-2 §2, key 10). The post office then restores `In-Reply-To` and `References` (RP-6 §3.1), so the recipient's mail program files the answer in its thread. A forward is a new message carrying the text and the `files`. |
| Status | Planned, not built: the client has no Reply or Forward and never sets `reply_to`. |
| Depends on | Nothing. |
| Size | Small: the reader and the composer; no protocol. |
| Priority | **P1.** Pilot users answer email by radio, and today each answer starts a new thread at the recipient's end. |
| Notes | Attachments held at the post office (RP-2 §2, `held`) cannot be forwarded until fetched (RP-6 §2.5): their `ref` means something only to the recipient's own post office. |

### W2. An address book and groups

Pick recipients by name. Write to a group (a net, a shelter team) in one step.

| | |
|---|---|
| Asked | The plan: concept §10, first version. 0 people. |
| Program | Client. |
| Compatibility | **Fits today.** The client's own; a group becomes addresses in `to` or `cc` (RP-2 §2). |
| Status | Planned, not built. |
| Depends on | Nothing. |
| Size | Medium: a store, its page in the panel, and import from the usual contact files. |
| Priority | **P2.** Everyone will want it early; nothing waits on it. |
| Notes | Every address goes on the air and onto the sender's accept list (RP-5 §6.5). A large group costs air time on every message. A list address at the post office, one address on the air, is the other way to do it, and would be a wish of its own (on top). |

### W3. Photos made small, and the time on the air shown

Attach a photo without thinking about its size. See before calling how long the outbox will take.

| | |
|---|---|
| Asked | The plan: concept §10, first version. 0 people. |
| Program | Client. |
| Compatibility | **Fits today.** Files travel in `files` (RP-2 §2). The post office already shrinks photos from email the same way (RP-6 §2.4, step 5: JPEG, 640 pixels on the long side). The estimate uses the rate the modem reports during a session (RP-4 §5, `quality`, `rate_bps`), and before one the rate of the last session. |
| Status | Planned, not built. |
| Depends on | Nothing. |
| Size | Medium: an image library in the client, and the estimate in the composer and the outbox. |
| Priority | **P2.** Saves air time for every user who attaches a photo; nothing waits on it. |
| Notes | Aether's rate runs from 36 bit/s to several kbit/s, so the estimate is a range until a session has measured the path. |

### W4. The full text of a trimmed message

Ask for the earlier messages a reply quoted, which the post office cut before sending it by radio.

| | |
|---|---|
| Asked | The plan: RP-2 §6 says a client SHOULD let the user ask for the full text. 0 people. |
| Program | Client, gateway, post office. |
| Compatibility | **Minor addition** to RP-3, RP-5 and RP-6. Nothing carries the request today: RP-3 §5.6's `REQUEST` asks for messages and held attachments, and RP-5 §5.3 for held attachments only. It needs an optional `REQUEST` key (the serials whose full text is wanted), an endpoint beside RP-5 §5.3's `files`, and RP-6 §2.5's way of answering: the full text as a new message from `postmaster@`. A gateway that does not know the key ignores it (RP-0 §4.1) and says nothing, so the client should say the request may not have been understood. |
| Status | New: RP-2 §6 promises it, with no way to ask (§12, G1). |
| Depends on | Nothing. |
| Size | Medium: three specifications, their vectors, three programs. |
| Priority | **P3.** No one has asked yet, and the trimmed part is usually the user's own earlier message. |
| Notes | A reading that needs no protocol covers most cases: the client shows the message `reply_to` names when it holds it, which it usually does, since the user wrote it. |

### W5. Mail kept as `.eml` files

Open the client's mail with any mail program, and back it up as ordinary files.

| | |
|---|---|
| Asked | The plan: concept §10 and design §5.1 say the client keeps mail as `.eml` files. 0 people. |
| Program | Client. |
| Compatibility | **Fits today.** RP-6 §3.1 says how a message becomes an email; the client can write the same file. The content ID still comes from the canonical encoding (RP-2 §3), so the client keeps that too, as design §5.1 says. |
| Status | Not built: the client keeps each message's canonical encoding, named by its content ID. The design and the program disagree (§12, G6). |
| Depends on | Nothing. |
| Size | Small for an export; medium to make `.eml` the store. |
| Priority | **P3.** Matters for trust and backups; no one has asked. |

### W6. Where each message is

See each sent message's state: written, sent by radio, waiting at a gateway, at the post office,
delivered to the recipient's mail server.

| | |
|---|---|
| Asked | The plan: concept §6.2. 0 people. |
| Program | Client; post office for the last state. |
| Compatibility | **Fits today** up to *at the post office*: a `RECEIPT` from a gateway that said `online` means the post office holds the message (RP-3 §5.5), and from a cut-off gateway that it is queued there (RP-7 §3.6). **Minor addition** for *delivered*: nothing tells the client that the recipient's server took it, and only failures come back, as a message (RP-6 §3.3). An optional client endpoint in RP-5 §6, asked over the Internet, costs no air time; a state carried by radio would. |
| Status | Partly built: the outbox and the sent folder. *Queued at* is Phase 3 (W29). *Delivered* has no mechanism (§12, G5). |
| Depends on | W29 for *queued at*. |
| Size | Medium. |
| Priority | **P2.** Users of a system built for outages will want to know where their mail is; *delivered* can wait. |

---

## 4. Accounts, setup and enrollment

### W7. Several accounts in one client

Work a personal address and a role mailbox (`shelter3@`) from one client.

| | |
|---|---|
| Asked | The plan: concept §10, first version. 0 people. |
| Program | Client. |
| Compatibility | **Fits today**, one session per account. Each operator of a role mailbox has a credential of their own (RP-1 §3), and a client can hold several. But `HELLO` carries one address and one signed statement (RP-3 §5.1, RP-1 §6.1), so each account is its own call. Both accounts in one session would be a **major change** to RP-1 and RP-3. |
| Status | Planned, not built: a client home holds one credential. |
| Depends on | Nothing. |
| Size | Medium: the home, the panel, the account chosen at each call. |
| Priority | **P2.** Role mailboxes are central to emergency work; a call per account will do at first. |
| Notes | A middle way, a **minor addition** to RP-3 §8: a link may carry a second session after the first ends, when both stations say they can (`caps`). It saves the modem's call and answer, not the session's five transmissions. |

### W8. The credential renewed by radio

Keep a credential valid with no Internet: the client asks for a new one during an ordinary session.

| | |
|---|---|
| Asked | The plan: RP-1 §4.2. 0 people. |
| Program | Client. |
| Compatibility | **Fits today**: `renew` in `HELLO`'s `caps` (RP-3 §5.1), passed to the post office (RP-5 §5.2), the new credential returned in `CREDENTIAL` (RP-3 §5.8). |
| Status | Partly built: the gateway asks the post office and hands the credential back, and the client keeps one it is sent; but the client never asks (it sends `renew` false). |
| Depends on | Nothing. |
| Size | Small: ask when fewer than a set number of days remain (30, say). |
| Priority | **P1.** A pilot user who reaches the post office only by radio loses the credential after 90 days, and an expired credential cannot be renewed: the device enrolls again (RP-5 §6.2). |

### W9. Usual gateways set in the panel

Name the gateways where copies of your mail should wait (concept §6.1).

| | |
|---|---|
| Asked | The plan: concept §10, first version. 0 people. |
| Program | Client. |
| Compatibility | **Fits today**: `usual_gateways` (RP-5 §6.3). |
| Status | Partly built: `radiopost settings --set` changes it on the command line; the panel has no place for it. |
| Depends on | W27: until copies are placed ahead, the setting does nothing. |
| Size | Small: a list in Setup, picked from the gateway list. |
| Priority | **P2**, with W27. |

### W10. Enrollment with a Logbook of the World certificate

Join radiopost.org without waiting for the sysop: prove the callsign with an LoTW certificate.

| | |
|---|---|
| Asked | The plan: D33. 0 people. |
| Program | Post office (its web page, or the client signing its challenge). |
| Compatibility | **Fits today**: how a post office checks a licence is its own business (RP-1 §1), and the check ends in the one-time code RP-5 §6.1 takes. If any client should do it with any post office, the challenge and its answer become a **minor addition** to RP-5 §6. |
| Status | Partly built: the sysop's check and the code are built (`radiopost-po code`); the LoTW path is not. |
| Depends on | Nothing. |
| Size | Medium: reading an LoTW certificate and checking it against the ARRL's. |
| Priority | **P2.** The sysop's check becomes the bottleneck once the pilot grows past a handful of testers. |

### W11. A trustee manages a role mailbox

The trustee of `shelter3@` authorises and removes the volunteers who may use it, without the post
office's sysop.

| | |
|---|---|
| Asked | The plan: concept §3.1, RP-1 §1. 0 people. |
| Program | Post office. |
| Compatibility | **Fits today**: accounts are the post office's business (RP-1 §1). Each operator still gets a credential with the `role` flag (RP-1 §4.1), and removing one is a revocation (RP-1 §8). |
| Status | Partly built: the sysop issues each operator's code with `--role`. |
| Depends on | Nothing. |
| Size | Medium: a trustee's page or commands, and who may act as trustee. |
| Priority | **P3**; rises when an emergency group runs a post office. |

### W12. Email-only members

Agency staff with no licence read and write the group's mail in an ordinary mail program, and reach
its radio users.

| | |
|---|---|
| Asked | The plan: concept §3.1. 0 people. |
| Program | Post office. |
| Compatibility | **Fits today**: they hold no credential and never transmit (RP-1 §1). Their mail to radio users is third-party traffic, flagged by the post office and checked by the gateway (RP-6 §5.1). |
| Status | Waits on W36: mail a mail-program user sends to another user of the same post office stays in the mail programs ([the post office guide](../user/post-office.md), §10). |
| Depends on | W36. |
| Size | Small once W36 is built. |
| Priority | **P2**, with W36. |

### W13. A callsign alias address

Receive mail at `kk4oda@radiopost.org` as well as at the chosen name.

| | |
|---|---|
| Asked | The plan: concept §3.1. 0 people. |
| Program | Post office. |
| Compatibility | **Fits today** for receiving: an alias in the mail server, judged at the door as the account it reaches (RP-6 §2.1). Sending *as* the alias would need a credential for that address, since the post office checks `from` against the statement's address (RP-5 §5.4); replies go out from the chosen name. |
| Status | Planned, not built. Whether Stalwart hands the door an alias as its account is still to be tried. |
| Depends on | Nothing. |
| Size | Small. |
| Priority | **P3.** |

---

## 5. Sessions and the radio

### W14. Choosing a gateway tunes the radio

Pick a gateway from the list, and the radio goes to its channel.

| | |
|---|---|
| Asked | The plan: concept §10, first version. 0 people. |
| Program | Client, adapters. |
| Compatibility | **Minor addition** to RP-4: an optional request to set and read the dial (say `tune`, with `dial_hz` and `mode`, and a `caps` member saying the adapter can). An adapter that cannot answers `unsupported`, as for any request it does not know (RP-4 §2). Aether can set and read its radio's dial; VARA's published commands cannot (RP-4 §9.2). The client could drive Aether's own control API beside RPMI today, but only with Aether, which is what RP-4 exists to avoid. |
| Status | Planned, not built: the panel says which channel to tune Aether to, and the operator tunes by hand. |
| Depends on | Nothing. W40 can use the same request to read the dial. |
| Size | Medium: RP-4 and its vectors, the Aether adapter, the client. |
| Priority | **P2.** Every call starts with it, and it removes a common mistake: calling on the wrong channel. |
| Notes | On a shared modem (RP-4 §1.1) a new dial moves every program on the station, so the adapter should refuse it (`busy`) while another program's link or call is up. RPMI carries no frequency at all today (§12, G3). |

### W15. Keep listening for stations

Leave the client answering station-to-station calls, not just the first.

| | |
|---|---|
| Asked | Found: [the on-air tests guide](../user/on-air-tests.md), §7: *Listen* answers one call, then stops. 0 people. |
| Program | Client. |
| Compatibility | **Fits today**: `listen` (RP-4 §4) stays on. |
| Status | New. |
| Depends on | Nothing. |
| Size | Small. |
| Priority | **P2.** Station-to-station tests are what testers do first. |
| Notes | A client answering with nobody at the radio is under automatic control. In the US that is allowed only in the §97.221(b) segments, since Aether's signals are wider than 500 Hz by the FCC's measure; Aether's gate enforces it (RP-4 §8). |

### W16. Scheduled connections

Call a gateway at set times to collect and send mail, unattended.

| | |
|---|---|
| Asked | The plan: concept §10, later. 0 people. |
| Program | Client. |
| Compatibility | **Fits today**: ordinary calls (RP-3). |
| Status | Planned, not built. |
| Depends on | W40, with any modem but Aether. |
| Size | Medium: a schedule, retries, and a report of what each call did. |
| Priority | **P3.** |
| Notes | A call with nobody at the radio is automatic control: in the US only in the automatic-control segments (§97.221(b); RP-7 §5.1 has the table). Aether refuses elsewhere; with another modem the client must check (RP-4 §8). |

### W17. Gateways ranked by propagation and by what the network has measured

The gateway list puts first the gateways the user can probably reach now, not just the nearest.

| | |
|---|---|
| Asked | The plan: concept §9 and §10, later. 0 people. |
| Program | Client; gateways and post offices for measurements. |
| Compatibility | **Fits today** for a prediction: the list gives each gateway's grid and channels (RP-5 §4.4), and a propagation model needs nothing more. **Minor addition** for measurements: an optional member of RP-5 §4.4, or a new public document, holding what gateways heard and from where (the SNRs their modems report, RP-4 §5). |
| Status | Planned, not built. Ranking by distance is built. |
| Depends on | Nothing, for a prediction. |
| Size | Medium for a prediction; large for measurements. |
| Priority | **P3.** |

### W18. Mail coming down compressed

Mail from the Internet takes less air time on its way down.

| | |
|---|---|
| Asked | Found while reading the programs. The concept counts on it: a 14 kB HTML email becomes about 1.1 kB (concept §5.1). 0 people. |
| Program | Post office. |
| Compatibility | **Fits today**: every station must take DEFLATE (RP-2 §4, code 1), and gateways pass the post office's air form on unchanged (RP-3 §7). |
| Status | New: the post office makes every air form uncompressed (code 0), as the reference that makes the test vectors does, so that they stay byte for byte. The client already sends its own mail compressed when that is smaller. |
| Depends on | Nothing. |
| Size | Small in the program. The vectors' downloads would change, or the program would compress where the vectors do not. |
| Priority | **P1.** Air time on every message a pilot user picks up. |
| Notes | RP-5 §5.3 does not say which compression the post office may use. It makes one air form for every gateway and never sees the caller's `caps`, so only codes 0 and 1 are safe for it (§12, G2). |

### W19. Shorter short messages: a published dictionary

Short messages, most of HF traffic, take a fifth less air time or better.

| | |
|---|---|
| Asked | The plan: D17, RP-2 §4; design §10 (the corpus is owed). 0 people. |
| Program | Client, gateway, post office. |
| Compatibility | **Minor addition** while it stays optional: code 3 is reserved, and each side lists the codes it takes (RP-3 §5.1, `caps`), so a station that does not know it is never sent it. RP-2 §4 says code 3 then *becomes a requirement*: that part is a **major change** once the specification is Stable (§12, G7). |
| Status | Planned (D17); Phase 1 shipped without it. |
| Depends on | W18, for any gain on mail coming down. |
| Size | Medium: write the corpus, train, measure against the 20 % rule, publish. |
| Priority | **P3.** It is a measurement before it is a feature. |
| Notes | Pulls against W18 and RP-5 §5.3: the post office makes one air form for every caller, so it can use code 3 only if every caller takes it (the requirement), or if it keeps an air form for each code, which breaks resuming a download through another gateway (RP-3 §7). |

### W20. A VARA HF and VARA FM adapter

Use RadioPost with the modem many stations already run.

| | |
|---|---|
| Asked | The plan: concept §7, design §5.4, Phase 5. 0 people. |
| Program | Adapter. |
| Compatibility | **Fits today**: RP-4 §9.2 maps VARA's published commands. VARA reports no delivery (RP-3 keeps its own receipts) and cannot be shared (`shared` false). It checks no band rules, so the gateway or its operator must (RP-4 §8, W40). |
| Status | Planned, Phase 5. |
| Depends on | W40 for an unattended gateway on VARA. |
| Size | Medium. |
| Priority | **P3** by the order of work; likely the adapter users ask for most. |
| Notes | VARA is a black box: the adapter uses only its published command list (RP-4 §9.2), nothing learned from its signal or its behaviour. A session over VARA cannot be read by an open monitor (W42), because VARA's air interface is not published. |

### W21. An ARDOP adapter

Use RadioPost over ARDOP.

| | |
|---|---|
| Asked | The plan: concept §7, design §5.4, Phase 5. 0 people. |
| Program | Adapter. |
| Compatibility | **Fits today**, by RP-4 §9.3, which is still to be checked against ARDOP's own specification (design §10). |
| Status | Planned, Phase 5. |
| Depends on | W40 for unattended use. |
| Size | Medium. |
| Priority | **P3.** |

### W22. An AX.25 packet adapter

Serve local users on VHF and UHF packet through Direwolf or a TNC, as the county EOC does in the
concept (N2CALL-5).

| | |
|---|---|
| Asked | The plan: concept §2 and §7, design §5.4, Phase 5. 0 people. |
| Program | Adapter. |
| Compatibility | **Fits today** through AGWPE (RP-4 §9.4). Whether it can be shared depends on how the TNC software treats a callsign two programs register (RP-4 §9.4). |
| Status | Planned, Phase 5. |
| Depends on | W26 to run beside HF on one gateway; W40 for unattended use. |
| Size | Medium. |
| Priority | **P3.** |

### W23. A phone version

Read, write and send by radio from a phone.

| | |
|---|---|
| Asked | The plan: concept §10, later; design §6; Phase 5. 0 people. |
| Program | Client. |
| Compatibility | **Fits today** over the Internet: enrollment and settings are RP-5 §6, and ordinary mail programs already read the mailbox (concept §5.4). By radio, the modem runs on a small computer at the radio. If the adapter runs on the phone and speaks that modem's own interface over the network, it fits today. If RPMI itself crosses from the phone to that computer, it is a **minor addition** to RP-4: §2 allows only the loopback interface or a Unix socket, and RPMI has no authentication, which a network would need (§12, G4). |
| Status | Planned, Phase 5. |
| Depends on | Nothing in RadioPost. Aether's own phone plan is on its back burner. |
| Size | Large. |
| Priority | **P3.** |

---

## 6. The gateway's operation

### W24. Block a caller by address or callsign

The gateway's operator refuses one account, or one station, that misuses the gateway.

| | |
|---|---|
| Asked | The plan: concept §9. 0 people. |
| Program | Gateway. |
| Compatibility | **Fits today**: `ERROR` `refused-by-policy` after `HELLO`, with a text saying why (RP-3 §5.9, RP-0 §9.3). |
| Status | Partly built: the operator can block a post office, or allow only some, from the command line (RP-1 §7.3); not an address or a callsign, and nothing in the panel. |
| Depends on | Nothing. |
| Size | Small. |
| Priority | **P2.** Operators need it before a gateway is widely known: §97.219(c) asks them to stop violating traffic once they know of it. |

### W25. The gateway renews its own credential

An unattended gateway keeps working past its credential's 90 days.

| | |
|---|---|
| Asked | The plan: RP-5 §6.2 lets a gateway renew. 0 people. |
| Program | Gateway. |
| Compatibility | **Fits today** (RP-5 §6.2). |
| Status | Partly built: the operator registers again, and the app warns two weeks ahead ([the gateway guide](../user/gateway.md), §2). |
| Depends on | Nothing. |
| Size | Small. |
| Priority | **P2.** An unattended gateway whose credential lapses stops carrying sessions. |

### W26. One gateway, several radios

One gateway program serves HF and VHF packet, or two HF bands, each on its own radio.

| | |
|---|---|
| Asked | The plan: design §5.2 (*Modem host*: one adapter per radio); the county EOC of concept §2. 0 people. |
| Program | Gateway. |
| Compatibility | **Fits today**: one RPMI connection per adapter (RP-4 §2), each channel in the gateway's record (RP-5 §4.4). |
| Status | Planned, not built: one modem per gateway. |
| Depends on | W22, for packet. |
| Size | Medium. |
| Priority | **P3**, with W22 and W30. |

---

## 7. Running without the Internet

The author has decided the order of these (CLAUDE.md, *Decided by the author*): copies placed
ahead, then the gateway's queue, then forwarding by radio.

### W27. Copies placed ahead

Mail waiting for a user is copied to their usual gateways, so it can be picked up while the
Internet or the post office is down.

| | |
|---|---|
| Asked | The plan: concept §6.1, D12, Phase 3. 0 people. |
| Program | Post office, gateway. |
| Compatibility | **Fits today**: specified in RP-5 §5.8 and §6.3, with the flag `copy` (RP-3 §5.2) and the serials of RP-3 §6.2. |
| Status | Specified, not built. |
| Depends on | W9, for users to name their gateways. |
| Size | Large: the long poll, the gateway's store, drops and expiry. |
| Priority | **P2**, first of Phase 3. **Order decided** by the author. |

### W28. The gateway's queue while offline, and its reports

A gateway that has lost the Internet still checks callers, takes their mail and serves its copies,
and reports everything when the Internet is back.

| | |
|---|---|
| Asked | The plan: concept §6.2, Phase 3. 0 people. |
| Program | Gateway, post office. |
| Compatibility | **Fits today**: RP-3 §9 and `online` (RP-3 §5.2), keys and revocation lists kept (RP-1 §7.3), `POST /reports` (RP-5 §5.6). |
| Status | Specified, not built: the gateway works only online. |
| Depends on | W27, for anything to serve. |
| Size | Large. |
| Priority | **P2**, second of Phase 3. **Order decided** by the author. |

### W29. The sender's client follows mail queued at a cut-off gateway

The user sees mail as queued at a gateway; it is offered again until a post office has it, and the
user is told if it may have been lost.

| | |
|---|---|
| Asked | The plan: D25. 0 people. |
| Program | Client. |
| Compatibility | **Fits today**: RP-7 §3.6. |
| Status | Specified, not built. |
| Depends on | W28. |
| Size | Medium. |
| Priority | **P2**, with W28. |

### W30. Forwarding by radio between gateways

Mail leaves an area that has lost the Internet, and comes back in, by radio between gateways.

| | |
|---|---|
| Asked | The plan: concept §6.3; D18, D21 to D23, D26; Phase 4. 0 people. |
| Program | Gateway, post office. |
| Compatibility | **Fits today**: RP-7, whose additions to RP-3 and RP-5 are already made. |
| Status | Specified, not built. |
| Depends on | W27, W28; W26 when the partner is reached on another radio; W41 for a gateway outside the US. |
| Size | Large. |
| Priority | **P3**, Phase 4. **Order decided** by the author: after the queue. |

### W31. Local delivery at a cut-off gateway

Neighbours who use the same cut-off gateway (a shelter and the EOC) exchange mail at once.

| | |
|---|---|
| Asked | The plan: D24. 0 people. |
| Program | Gateway; client (it asks for local messages by content ID, RP-3 §5.6, `ids`). |
| Compatibility | **Fits today**: RP-7 §4.4. |
| Status | Specified, not built. |
| Depends on | W27, W28. |
| Size | Medium. |
| Priority | **P3**, with W30. |

---

## 8. Forms and applications

### W32. Forms

Fill in ICS-213 and other public forms. The recipient sees the form; an email reader gets a
readable copy.

| | |
|---|---|
| Asked | The plan: concept §10, later; design §9, Phase 5; RP-2 §2 reserves key 13 for forms. 0 people. |
| Program | Client; post office for email; a new specification. |
| Compatibility | **Minor addition**, as RP-2 plans: a forms specification defines key 13, and every form message also carries its text in `body`, so a client that knows no forms still shows it (receivers keep keys they do not know, RP-2 §2). RP-6 then says how a form becomes email and back. **On top** is the other way: a form as an attachment of a published type, nothing changed below; but the post office could not make a readable email of it, and key 13 would stay unused. Choose before building. |
| Status | Planned, Phase 5; key 13 reserved. |
| Depends on | Nothing. |
| Size | Large: a template format, a set of forms, the client's editor and viewer, RP-6's conversion. |
| Priority | **P3** by the order of work; likely to rise, since emergency groups ask for forms first. |
| Notes | Templates come from FEMA's public forms, in an open format (concept §10), not from Winlink's: no compatibility with Winlink's programs, by the author's decision. |

### W33. Position reports

Send where a station is, for a map or a net.

| | |
|---|---|
| Asked | The plan: concept §10, later; RP-4 §6 reserves broadcasts for it. 0 people. |
| Program | Client; a service that collects them, or a new specification. |
| Compatibility | **On top** as a message to a position service's address: fits today, at the cost of a session each. As a broadcast heard by everyone listening: a new specification, a **minor addition** carried only by modems with `caps.broadcast` (RP-4 §6). |
| Status | Planned, not built. |
| Depends on | Nothing. |
| Size | Medium. |
| Priority | **P3.** |
| Notes | Whether a station under automatic control may send such a broadcast must be checked against the rules before it is designed. |

### W34. Bulletins

Read a net's or an agency's bulletins.

| | |
|---|---|
| Asked | The plan: concept §10, later; RP-4 §6. 0 people. |
| Program | On top (a list address), or a new specification; client. |
| Compatibility | **On top** as mail from a list address: fits today, but every reader downloads their own copy. As a broadcast: one transmission for everyone listening, a new specification (**minor addition**), and no acknowledgement, so a missed bulletin still needs the mail copy. |
| Status | Planned, not built. |
| Depends on | Nothing for mail; W33's broadcast format if both use one. |
| Size | Medium for mail; large for broadcasts. |
| Priority | **P3.** |
| Notes | Air time pulls one way and simplicity the other: the broadcast saves air on a busy net, and costs a specification and a second way to get the same bulletin. |

### W35. Services that answer by mail

Write to an address for a catalogue, a weather report or a blank form, and get the answer as mail.

| | |
|---|---|
| Asked | The plan: concept §10, later (catalogue requests). 0 people. |
| Program | On top: a service at a post office, or anywhere on the Internet. |
| Compatibility | **On top**: ordinary mail both ways. Writing to the service puts it on the accept list (RP-5 §6.5). |
| Status | Planned, not built. |
| Depends on | Nothing. |
| Size | Small for each service. |
| Priority | **P3.** |
| Notes | An answer that arrives as ordinary email is third-party mail (RP-6 §5.1), held back from stations in countries without an arrangement; one a RadioPost account sends, with RP-6 §4's part, is not. |

---

## 9. The post office

### W36. One mailbox, by radio and by mail program

Mail read in a mail program is not offered again by radio, mail sent by radio shows in the mail
program's Sent folder, and mail between users of the same post office reaches them whichever way
they read.

| | |
|---|---|
| Asked | The plan: concept §5.4; RP-5 §6.3 (`radio_skips_read`). 0 people. |
| Program | Post office. |
| Compatibility | **Fits today.** |
| Status | Partly built: the two mailboxes are separate ([the post office guide](../user/post-office.md), §10). |
| Depends on | Nothing. |
| Size | Large: JMAP both ways, and every path mail takes inside the post office. |
| Priority | **P2.** Pilot users with the Internet will notice at once, and W12 waits on it. |

### W37. The accept list on a web page

Add or remove senders from any browser.

| | |
|---|---|
| Asked | The plan: concept §5.1. 0 people. |
| Program | Post office. |
| Compatibility | **Fits today**: the post office's own page over the same list (RP-5 §6.5, RP-6 §2.2). |
| Status | Planned, not built: the client changes the list. |
| Depends on | A login for the page; the user's mail-server account could serve. |
| Size | Medium. |
| Priority | **P3.** |

### W38. Backups and a backup mail server

A post office that loses its server loses no mail, and is rebuilt in hours.

| | |
|---|---|
| Asked | The plan: concept §8.5; the pilot is to have nightly off-site backups and a backup mail server at a second company (concept §12). 0 people. |
| Program | Post office. |
| Compatibility | **Fits today**: one operator's servers; no protocol. |
| Status | Planned, not built: the post office guide sets up one server. |
| Depends on | Nothing. |
| Size | Medium. |
| Priority | **P1** for the backups: the pilot holds real users' mail. The backup mail server can follow. |
| Notes | A backup mail server must refuse as the door does, or it later sends bounces to forged senders (D15, RP-6 §2.1); so it needs the accept lists, which concept §13 left open. With none, senders retry for days (RFC 5321), and only time is lost. |

### W39. A domain served by several post offices

An address that survives any one post office closing.

| | |
|---|---|
| Asked | The plan: concept §12, a later option; concept §1 lists it among what is not a goal for now. 0 people. |
| Program | Post office; gateways and clients. |
| Compatibility | **Major change**: RP-1 §7.1 allows exactly one API record per domain, and serials (RP-3 §6.2) are one post office's numbering, on which a client's `cursor` relies. |
| Status | A later option, in no phase. |
| Depends on | Nothing. |
| Size | Large. |
| Priority | **P4.** |

---

## 10. The operator's safety and the rules

### W40. The gateway checks the band rules itself

With a modem that checks nothing (VARA, ARDOP, packet), an unattended gateway still keeps to the
automatic-control segments.

| | |
|---|---|
| Asked | The plan: design §5.2 (*Rules check*), concept §7; RP-4 §8 requires it of the host or its operator. 0 people. |
| Program | Gateway; the client too, for W16. |
| Compatibility | **Fits today** with the dial taken from the gateway's own settings or its registered channels (RP-5 §4.4), judged against its country's table (RP-7 §5.1). Reading the radio's real dial from the modem is a **minor addition** to RP-4, since RPMI carries no frequency (W14; §12, G3). |
| Status | Partly built: a setting says the operator checks ([the gateway guide](../user/gateway.md), §2); the gateway checks nothing itself. |
| Depends on | W14, to read the dial; W41 outside the US. |
| Size | Medium. |
| Priority | **P3**: needed before W20 to W22 go unattended. |

### W41. Rules tables for other countries

A gateway outside the US hands over third-party mail, and forwards unattended, by its own country's
rules.

| | |
|---|---|
| Asked | The plan: design §10; RP-6 §5.1, RP-7 §5.1, §8. 0 people. |
| Program | Gateway; the specifications. |
| Compatibility | **Minor addition**: a country's table in RP-6 §5.1 or RP-7 §5.1 changes nothing for gateways elsewhere. Each is read from that country's rules, with the date. |
| Status | Planned: the United States only. Elsewhere the operator enters prefixes and segments (RP-6 §5.1, RP-7 §5.1). |
| Depends on | Someone reading each country's rules. |
| Size | Medium for each country. |
| Priority | **P3**; rises when a gateway outside the US joins. |

### W42. A monitor that decodes sessions off the air

Anyone can record a RadioPost session and read every message in it with an open tool.

| | |
|---|---|
| Asked | The plan: concept §11, *Monitoring*. 0 people. |
| Program | On top: a tool. |
| Compatibility | **Fits today**: everything on the air is published (RP-0 §7; RP-2 §4; RP-3 §2, §4), and so is Aether's air interface. |
| Status | Planned, not built. |
| Depends on | Aether giving a session's bytes from a recording. |
| Size | Medium. |
| Priority | **P2.** It is the concept's answer to the "obscured meaning" complaint, and a debugging tool for every implementer. |
| Notes | Over VARA it cannot exist: VARA's air interface is not published. |

---

## 11. For other implementers

### W43. A public test service

Someone writing their own client, gateway or adapter tests it against RadioPost's tester over the
Internet, and on the air against a test gateway.

| | |
|---|---|
| Asked | The plan: [the conformance tester](01-conformance.md), §7 (not settled). 0 people. |
| Program | Tools. |
| Compatibility | **Fits today**: the tester exists (`core/tester`), and the vectors are published (RP-0 §8.6). |
| Status | Not settled. |
| Depends on | Real keys for an on-air test gateway: the published test keys never go on the air (the tester's §2.3). |
| Size | Medium. |
| Priority | **P3**; rises with the first independent implementation, which RP-0 §8.1 needs before any specification becomes Candidate. |

---

## 12. Gaps found in the specifications and the design

Each is noted under its wish, and kept here so that none is lost. Nothing is changed in this
document: a gap is fixed, if at all, through the specification's own process.

| # | Where | What | Wish |
|---|---|---|---|
| G1 | RP-2 §6 | A client SHOULD let the user ask for a trimmed message's full text, but no PDU or endpoint carries the request (RP-3 §5.6, RP-5 §5.3). | W4 |
| G2 | RP-5 §5.3 | It does not say which compression the post office's air form may use. RP-2 §4 has a sender choose one both ends take, but the post office makes one air form for every gateway and never sees the caller's `caps`, so only codes 0 and 1 are safe. Both the reference and the program use 0. | W18, W19 |
| G3 | RP-4 | RPMI carries no frequency. RP-4 §8 has the host check the band rules when the modem does not, but gives it no way to learn the dial, and the client cannot tune. | W14, W40 |
| G4 | RP-4 §2 | The adapter listens on the loopback interface or a Unix socket, and RPMI has no authentication, so it cannot cross from a phone to the computer at the radio. | W23 |
| G5 | Concept §6.2 | Promises *delivered to the recipient's provider*; nothing in RP-5 or RP-6 reports it. | W6 |
| G6 | Design §5.1 | Says the client keeps mail as `.eml` files; the client keeps the canonical encoding. | W5 |
| G7 | RP-2 §4 | Dictionary 1 "becomes a requirement" once adopted: a change others cannot ignore, where a negotiated option would not be one. | W19 |

---

## 13. Set aside

Wishes refused once, with the reason, so they are not argued again from the start. It starts with
what the concept and the design have already turned down.

| Wish | Why not | Decided in |
|---|---|---|
| Work with Winlink's programs, or speak B2F | A client and protocol of RadioPost's own, by choice. | Concept §1; [CLAUDE.md](../../CLAUDE.md), *Decided by the author* |
| Encrypted or private mail on the air | The rules forbid messages encoded to obscure their meaning (§97.113(a)(4)). RadioPost signs and never encrypts. | RP-0 §5, §7; concept §1 |
| Email for people who are not amateurs | RadioPost is not an email provider for the public. Email-only members of a group (W12) are different: their mail reaches radio users as third-party traffic. | Concept §1 |
| A list of post offices kept by the project | A gatekeeper, against the no-centre design. Gateways trust what DNS shows, and their operators decide. | D9, D32; CLAUDE.md, *Decided by the author* |
| Gateways that announce what they reach | An announcement would have to be trusted, and one false one draws an area's mail away. | D23 |
| The gateway sending email itself | It cannot sign for the user's domain, so the mail is filtered or refused. | D16 |
| Taking mail and bouncing it later | Bounces go to forged senders. | D15 |
| A dictionary trained on real mail | A dictionary is published and keeps what it learned verbatim. | D17; CLAUDE.md, *Decided by the author* |
| A password sent over the air | Nothing secret goes over the air; signatures prove who is calling. | Concept §3.3 |

---

## Change log

| Version | Date | Change |
|---|---|---|
| 0.1.0 | 2026-11-10 | First draft: how a wish is recorded; seeded with what the concept and the design promise and have not built, and two things found in the programs (W1 to W43); the gaps found on the way (G1 to G7); what the design has already set aside. |
