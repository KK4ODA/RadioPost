# RadioPost: the wishlist

| | |
|---|---|
| **Status** | Draft |
| **Version** | 0.2.4 |
| **Updated** | 2026-11-11 |
| **Editor** | KK4ODA |
| **Licence** | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| **Follows** | [The detailed design](00-detailed-design.md), §9 and §10 |

What the author and users ask the client, the gateway, the post office, the adapters and the
applications built on RadioPost to do, in one list: grouped, prioritized, checked against the
specifications, and judged. A wish is listed with a recommendation, which may be to build it, to
build something better, to fold it into another, to wait, or not to build it.

RadioPost is a common protocol that other people will write their own programs for. A feature that
works only in our programs, or that needs a protocol change other implementers cannot ignore,
costs everyone. So every wish says what it would ask of the specifications before anyone decides
to do it.

The list starts with what the [concept](../concept/00-concept.md) and the detailed design promise
and have not built, so that new wishes land in a list that already knows the plan. Nothing is built
because it is listed, and no specification changes because of this list: a wish taken up goes
through the specifications' own process (RP-0 §8.4).

---

## 1. How to read it

Each wish has an ID (W1, W2, …) that it keeps for good, wherever it moves. A wish that repeats one
already listed is merged into it, and its count of requesters rises; a wish folded into another
keeps its ID in the summary, pointing to the wish it joined. Under its name, one or two sentences
say what the user gets. A table gives the rest:

| Row | Holds |
|---|---|
| Requested by | *The plan* (the concept, the design or a specification promises it), the author, users by callsign or name, or *the editor* (proposed while keeping this list: not a request, and never counted as demand). Then the number of independent requesters. |
| Component | Client, gateway, post office, adapter, application layer, or several. |
| Compatibility | One of the verdicts below, with the sections it rests on. |
| Existing coverage | Planned (with its phase or decision), partly built, specified but not built, or new. |
| Dependencies | Other wishes, or phases of the order of work (design §9). |
| Effort | Small: days, one program, no protocol. Medium: weeks, several programs, or an optional addition with its test vectors. Large: a phase's worth, or a protocol change through several programs. |
| Priority | Critical, High, Medium or Low, with the reason. |
| Recommendation | Accept, modify, consolidate, defer, or recommend against. |
| Status | Proposed, under review, accepted, planned, in progress, implemented, deferred or rejected. |
| Author decision | The author's own call, with its date and source. A decided point is not reopened by a later reshuffle, only by new evidence. |
| Notes | Alternatives, conflicts, uncertainties, what it adds to the complexity budget (§2). |

**Compatibility verdicts.** RP-0 §8.2 sets the kinds of change. While the specifications are
Drafts, every change to a requirement raises only their MINOR number, so a verdict names the kind
of change by its nature: whether a program written to today's text keeps working.

| Verdict | Means |
|---|---|
| **Fits today** | No protocol change; the sections it relies on are named. |
| **Optional protocol addition** | An addition other implementations can ignore: an optional key, request, endpoint or code (RP-0 §8.2, MINOR). |
| **Breaking protocol change** | A change other implementations cannot ignore (RP-0 §8.2, MAJOR); what breaks is named. |
| **Application-layer capability** | An application carried in ordinary RadioPost messages, with nothing changed below it. |
| **Conflict** | With the amateur rules, a design decision, the author's decisions in CLAUDE.md, another wish, or a protocol guarantee; the conflict is named, and a compliant way to meet the need is looked for. |
| **Needs specification review** | The specifications are silent or ambiguous on the point, so no verdict can be given without inventing one. |

Statements about the rules say whether they come from a rule's text, from a RadioPost or Aether
document's reading of it, or need checking. None of this is legal advice.

**Priorities.** No scores: each priority has its reason.

| | Means |
|---|---|
| **Critical** | Without it, mail or a user's standing is at risk, or the pilot cannot run safely. |
| **High** | It would earn one of the next release's three slots: the pilot's users need it, or it is the next phase's core. |
| **Medium** | Wanted and worth doing; nothing urgent waits on it. |
| **Low** | Later, or until users show a need. |

---

## 2. Principles

The wishes are judged against what the project has already decided. These are reminders, not new
rules:

1. **The specifications are the contract** (design §1, RP-0 §3). A feature only our programs can
   use is weighed as a cost to everyone else's.
2. **The protocol stays small and stable.** In order of preference: improve what exists; change a
   program's own behaviour or defaults; an application carried in ordinary messages; an optional
   addition others can ignore (RP-0 §8.2); and only for a benefit that pays for it, a change others
   cannot ignore.
3. **Small on the air, few turnarounds** (design §2, principles 3 and 4). Air time is the scarcest
   thing RadioPost has; a feature that adds bytes or exchanges to every session must earn them.
4. **Open on the air** (design §2, principle 2; RP-0 §7). Nothing encrypted, every encoding
   published.
5. **Nobody guesses, and no mail is silently lost** (design §2, principle 7; D25). Reliability and
   the integrity of mail come before convenience.
6. **Works without the Internet where it matters** (design §2, principle 6; concept goal G5).
7. **Federated, with no centre** (design §2, principle 8; D9, D32).
8. **Simple by default.** An occasional operator sends and receives with no settings to learn;
   expert options stay out of the way until wanted.
9. **Complexity has a budget.** A substantial wish says what it adds to screens, modes, settings,
   tests, documents and the protocol, and whether something else can be simplified in return.

---

## 3. Summary

| ID | Wish | Group | Priority | Compatibility | Status |
|---|---|---|---|---|---|
| W1 | Reply and forward | Mail | High | Fits today | Planned |
| W2 | An address book and groups | Mail | Medium | Fits today | Planned |
| W3 | Photos made small, and the time on the air shown | Mail | Medium | Fits today | Planned |
| W4 | The full text of a trimmed message | Mail | Low | Fits today as modified; optional addition as asked | Proposed |
| W5 | Mail kept as `.eml` files | Mail | Low | Fits today | Planned |
| W6 | Where each message is | Mail | Medium | Fits today; optional addition for *delivered* | Planned |
| W7 | Several accounts in one client | Setup | Medium | Fits today, a call each | Planned |
| W8 | Credentials renewed before they lapse | Setup | High | Fits today | Implemented |
| W9 | Usual gateways set in the panel | — | — | — | Consolidated into W27 |
| W10 | Enrollment with an LoTW certificate | Setup | Medium | Fits today | Planned |
| W11 | A trustee manages a role mailbox | Setup | Low | Fits today | Planned |
| W12 | Email-only members | — | — | — | Consolidated into W36 |
| W13 | A callsign alias address | Setup | Low | Fits today | Planned |
| W14 | Choosing a gateway tunes the radio | Radio | Medium | Optional addition (RP-4) | Planned |
| W15 | Keep listening for stations | Radio | Medium | Fits today | Proposed |
| W16 | Scheduled connections | Radio | Low | Fits today | Planned |
| W17 | Gateways ranked by propagation | Radio | Low | Fits today; optional addition for measurements | Planned |
| W18 | Mail coming down compressed | Radio | High | Fits today | Proposed |
| W19 | Shorter short messages: a dictionary | Radio | Low | Optional addition; breaking if made required | Planned |
| W20 | An adapter for VARA-style modems: VARA and Mercury | Adapters | Medium | Fits today; G11 to name Mercury | Planned |
| W21 | An ARDOP adapter | Adapters | Low | Needs specification review | Planned |
| W22 | An AX.25 packet adapter | Adapters | Low | Fits today | Planned |
| W23 | A phone version | Radio | Low | Fits today; optional addition for RPMI across devices | Planned |
| W24 | Block a caller by address or callsign | Operation | Medium | Fits today | Planned |
| W25 | The gateway renews its own credential | — | — | — | Consolidated into W8 |
| W26 | One gateway, several modems | Operation | Medium | Fits today; G11 to name Mercury | Planned |
| W27 | Copies placed ahead | Offline | High | Fits today | Planned |
| W28 | The gateway's queue while offline, and its reports | Offline | High | Fits today | Planned |
| W29 | The sender's client follows queued mail | Offline | High | Fits today | Planned |
| W30 | Forwarding by radio | Offline | Medium | Fits today | Planned |
| W31 | Local delivery at a cut-off gateway | Offline | Medium | Fits today | Planned |
| W32 | Forms | Applications | Medium | Application layer (recommended); optional addition (RP-2 key 13) | Planned |
| W33 | Position reports | Applications | Low | Application layer; optional addition as broadcasts | Planned |
| W34 | Bulletins | Applications | Low | Application layer; optional addition as broadcasts | Planned |
| W35 | Services that answer by mail | Applications | Low | Application layer | Planned |
| W36 | One mailbox, by radio and by mail program | Operation | High | Fits today | Planned |
| W37 | The accept list on a web page | Operation | Low | Fits today | Planned |
| W38 | Backups and a backup mail server | Operation | Critical | Fits today | Planned |
| W39 | A domain served by several post offices | — | — | Breaking (RP-1, RP-3) | Deferred |
| W40 | The gateway checks the band rules itself | Rules | Medium | Fits today; optional addition to read the dial | Planned |
| W41 | Rules tables for other countries | Rules | Low | Optional addition (RP-6, RP-7) | Planned |
| W42 | A monitor that decodes sessions off the air | Rules | Medium | Fits today | Planned |
| W43 | A public test service | Adapters | Low | Fits today | Proposed |
| W44 | One install, several radios | Setup | Medium | Fits today; needs specification review for a portable callsign | Proposed |

Groups: *Mail* (§4), *Setup* (§5), *Radio* (§6), *Operation* (§7), *Offline* (§8),
*Applications* (§9), *Adapters* (§10), *Rules* (§11). Consolidated and deferred wishes are in §14.

---

## 4. Composing and reading mail

### W1. Reply and forward

Answer a message and keep its thread, at the other end too. Pass a message on to someone else.

| | |
|---|---|
| Requested by | The plan: concept §10, first version. Requesters: 0. |
| Component | Client. |
| Compatibility | **Fits today.** A reply sets `reply_to` (RP-2 §2, key 10); the post office restores `In-Reply-To` and `References` from it (RP-6 §3.1), so the recipient's mail program threads the answer. A forward is a new message with the text and the `files`. |
| Existing coverage | Planned for the first version; not built. The client never sets `reply_to`. |
| Dependencies | None. |
| Effort | Small: the reader and the composer. |
| Priority | **High.** Pilot users answer email by radio, and today each answer starts a new thread at the other end. |
| Recommendation | Accept. |
| Status | Planned. |
| Author decision | None yet. |
| Notes | Held attachments cannot be forwarded until fetched (RP-6 §2.5): their `ref` means something only to the recipient's own post office. |

### W2. An address book and groups

Pick recipients by name. Write to a group (a net, a shelter team) in one step.

| | |
|---|---|
| Requested by | The plan: concept §10, first version. Requesters: 0. |
| Component | Client. |
| Compatibility | **Fits today.** The client's own; a group becomes addresses in `to` or `cc` (RP-2 §2). |
| Existing coverage | Planned; not built. |
| Dependencies | None. |
| Effort | Medium: a store, its page in the panel, and import of the usual contact files. |
| Priority | **Medium.** Everyone will want it; nothing waits on it. |
| Recommendation | Accept, as a client feature. |
| Status | Planned. |
| Author decision | None yet. |
| Notes | Every address goes on the air and onto the accept list (RP-5 §6.5). If groups grow large, a list address at the post office (one address on the air) is the cheaper way, and an application-layer wish of its own. |

### W3. Photos made small, and the time on the air shown

Attach a photo without thinking about its size. See, before calling, how long the outbox will take.

| | |
|---|---|
| Requested by | The plan: concept §10, first version. Requesters: 0. |
| Component | Client. |
| Compatibility | **Fits today.** Files travel in `files` (RP-2 §2). The estimate uses the rate the modem reports (RP-4 §5, `quality`, `rate_bps`), and before a session the last session's rate. |
| Existing coverage | Planned; not built. |
| Dependencies | None. |
| Effort | Medium: an image library in the client; the estimate in the composer and the outbox. |
| Priority | **Medium.** Saves air time for everyone who attaches a photo. |
| Recommendation | Accept, shrinking by the post office's own rule (RP-6 §2.4, step 5: JPEG, 640 pixels on the long side), so photos look the same whichever way they travel. |
| Status | Planned. |
| Author decision | None yet. |
| Notes | Aether runs from 36 bit/s to several kbit/s, so the estimate is a range until a session has measured the path. |

### W4. The full text of a trimmed message

See the earlier messages a reply quoted, which the post office cut before sending it by radio.

| | |
|---|---|
| Requested by | The plan: RP-2 §6 says a client SHOULD let the user ask for the full text. Requesters: 0. |
| Component | Client; as asked, also the gateway and the post office. |
| Compatibility | As asked, an **optional protocol addition** to RP-3, RP-5 and RP-6: nothing carries the request today (§13, G1). As modified below, **fits today**. |
| Existing coverage | Promised by RP-2 §6, with no way to ask. |
| Dependencies | None. |
| Effort | Small as modified; medium as asked (three specifications, their vectors, three programs). |
| Priority | **Low.** No one has asked. |
| Recommendation | **Modify.** What a reply cut is usually the user's own earlier message, which the client holds: show the message `reply_to` names from the client's store. Ask for a protocol addition only if users miss history they do not hold. |
| Status | Proposed. |
| Author decision | None yet. |

### W5. Mail kept as `.eml` files

Open the client's mail in any mail program, and back it up as ordinary files.

| | |
|---|---|
| Requested by | The plan: concept §10 and design §5.1 say the client keeps mail as `.eml`. Requesters: 0. |
| Component | Client. |
| Compatibility | **Fits today.** RP-6 §3.1 says how a message becomes an email. |
| Existing coverage | Not built: the client keeps each message's canonical encoding (RP-2 §3). The design and the program disagree (§13, G6). |
| Dependencies | None. |
| Effort | Small for export and import; medium to change the store. |
| Priority | **Low.** Matters for trust and backups; no one has asked. |
| Recommendation | **Modify**: export and import `.eml`, keep the canonical store (the content ID comes from it), and have design §5.1 say so. Changing the design is the author's call. |
| Status | Planned. |
| Author decision | None yet. |

### W6. Where each message is

See each sent message's state: written, sent by radio, waiting at a gateway, at the post office,
delivered to the recipient's mail server.

| | |
|---|---|
| Requested by | The plan: concept §6.2. Requesters: 0. |
| Component | Client; the post office for *delivered*. |
| Compatibility | **Fits today** up to *at the post office*: a `RECEIPT` from a gateway that said `online` means the post office holds it (RP-3 §5.5); from a cut-off gateway, that it is queued (RP-7 §3.6). *Delivered* is an **optional protocol addition**: nothing reports it, and only failures come back, as messages (RP-6 §3.3; §13, G5). |
| Existing coverage | Partly built: the outbox and the sent folder. |
| Dependencies | W29 for *queued at a gateway*. |
| Effort | Small for the states the radio reports; medium with *delivered*. |
| Priority | **Medium.** Users of a system built for outages need to know where their mail is. |
| Recommendation | **Modify**: show what the radio already reports; leave *delivered* until users ask, since a failure already arrives as a message. |
| Status | Planned. |
| Author decision | None yet. |

---

## 5. Setup, accounts and configuration

### W7. Several accounts in one client

Work a personal address and a role mailbox (`shelter3@`) from one client.

| | |
|---|---|
| Requested by | The plan: concept §10, first version. Requesters: 0. |
| Component | Client. |
| Compatibility | **Fits today**, one call per account: each operator of a role mailbox holds their own credential (RP-1 §3), but `HELLO` carries one address and one statement (RP-3 §5.1, RP-1 §6.1). Several accounts in one session is a **breaking protocol change** to RP-1 and RP-3. |
| Existing coverage | Planned; not built. A client home holds one credential. |
| Dependencies | None. |
| Effort | Medium: the home, the panel, choosing the account for each call. |
| Priority | **Medium.** Role mailboxes are central to emergency work; a call each will do at first. |
| Recommendation | Accept, a call per account. Recommend against several accounts in one session (§14). |
| Status | Planned. |
| Author decision | None yet. |
| Notes | Whether a second session may follow the first on the same link, saving the modem's call, is not said (RP-3 §8; §13, G9). |

### W8. Credentials renewed before they lapse

Nobody has to enroll again because a date passed: the client renews by radio or Internet,
whichever it reaches first, and the gateway renews its own.

| | |
|---|---|
| Requested by | The plan: RP-1 §4.2; RP-5 §6.2 (a gateway renews too). Requesters: 0. Includes W25. |
| Component | Client, gateway. |
| Compatibility | **Fits today.** By radio: `renew` in `HELLO`'s `caps` (RP-3 §5.1), passed on (RP-5 §5.2), returned in `CREDENTIAL` (RP-3 §5.8). Over the Internet: RP-5 §6.2, for clients and gateways. |
| Existing coverage | Built (2026-11-11). The rule is `radiopost::credential::RENEW_WITHIN` (30 days) and `renewal_due`. The client's panel renews over the Internet when it starts and every hour, between sessions, and a gateway call asks with `renew` in `HELLO`; the gateway renews its own while it answers, from its answering loop, and takes the new one up before its next session. Each says a failure once until it changes. Before: the gateway passed `renew` on and the client kept a credential it was sent, but never asked; the client renewed over the Internet only when told to, and the gateway was registered again by hand. |
| Dependencies | None. |
| Effort | Small. |
| Priority | **High.** An expired credential cannot be renewed (RP-5 §6.2): a radio-only user must re-enroll through the Internet and the sysop, and an unattended gateway stops carrying sessions. |
| Recommendation | Accept, with one rule for every credential: once under 30 days remain, renew at the first chance by whatever path is open; tell the user only when that keeps failing. |
| Status | Implemented. |
| Author decision | Build it (2026-11-11), after the author found the client's Setup said nothing of the credential's date. |

### W10. Enrollment with a Logbook of the World certificate

Join radiopost.org without waiting for the sysop: prove the callsign with an LoTW certificate.

| | |
|---|---|
| Requested by | The plan: D33. Requesters: 0. |
| Component | Post office; the client or a web page signs the challenge. |
| Compatibility | **Fits today**: a post office checks licences its own way (RP-1 §1), ending in the code RP-5 §6.1 takes. An **optional protocol addition** to RP-5 §6 only if every client should do it with every post office. |
| Existing coverage | Partly built: the sysop's check and the code (`radiopost-po code`). |
| Dependencies | None. |
| Effort | Medium: reading an LoTW certificate and checking it against the ARRL's. |
| Priority | **Medium.** The sysop becomes the bottleneck once the pilot passes a handful of testers. |
| Recommendation | Accept, as the post office's own, outside RP-5. |
| Status | Planned. |
| Author decision | **Decided** (2026-11-09, D33, CLAUDE.md): radiopost.org enrolls on an LoTW certificate or the sysop's check. |

### W11. A trustee manages a role mailbox

The trustee of `shelter3@` adds and removes the volunteers who may use it, without the sysop.

| | |
|---|---|
| Requested by | The plan: concept §3.1, RP-1 §1. Requesters: 0. |
| Component | Post office. |
| Compatibility | **Fits today**: accounts are the post office's business (RP-1 §1); each operator gets a credential with `role` (RP-1 §4.1), and removing one is a revocation (RP-1 §8). |
| Existing coverage | Partly built: the sysop issues each operator's code with `--role`. |
| Dependencies | None. |
| Effort | Medium: the trustee's commands or page, and who counts as trustee. |
| Priority | **Low** until an emergency group runs a post office. |
| Recommendation | Defer until then. |
| Status | Planned. |
| Author decision | None yet. |

### W13. A callsign alias address

Receive mail at `kk4oda@radiopost.org` as well as at the chosen name.

| | |
|---|---|
| Requested by | The plan: concept §3.1. Requesters: 0. |
| Component | Post office. |
| Compatibility | **Fits today** for receiving: an alias in the mail server, judged at the door as its account (RP-6 §2.1). Sending *as* the alias needs a credential for it, since `from` must be the statement's address (RP-5 §5.4). |
| Existing coverage | Planned; not built. Whether Stalwart hands the door an alias as its account is to be tried. |
| Dependencies | None. |
| Effort | Small. |
| Priority | **Low.** |
| Recommendation | Accept, receiving only. |
| Status | Planned. |
| Author decision | None yet. |

### W44. One install, several radios

Use one RadioPost and one Aether with every radio on the computer. Pick the radio, and everything
that depends on it follows, instead of keeping a copy of the program for each radio as Winlink
users do.

| | |
|---|---|
| Requested by | The author (KK4ODA), 2026-11-11. Requesters: 1. |
| Component | Client and the Aether adapter; Aether itself. For a gateway, W26. |
| Compatibility | **Fits today.** What differs between radios is the modem's business (RP-4 §1: keying, frequencies, modulation), and Aether keeps it in profiles. The rest is the client's own: which modem it uses and what it shows. Switching Aether's profile from RadioPost goes through Aether's own control API inside the Aether adapter, outside RPMI, so it obliges no one else. A profile that operates under another callsign (`KK4ODA/M`, `VE3/KK4ODA`) **needs specification review** (§13, G10). |
| Existing coverage | Partly built, on Aether's side. Its profiles (Aether's ADR-0011) are made for "two radios, or a base and a truck": one per radio, with its sound card, keying, bandwidth, drive and dials. Under Aether's app, a profile that changes the sound card or the keying port restarts the modem by itself in a few seconds. RadioPost has one *Modem* field and nothing per radio. |
| Dependencies | W7 (accounts stay separate from radios); W26 for a gateway; T7 (§12). |
| Effort | Small for a guide and for showing which radio is active; medium to switch from RadioPost and to list only the gateways the radio can reach. |
| Priority | **Medium.** Common among operators with HF and VHF, or a base and a mobile; most of it already works, and few will know it. |
| Recommendation | **Modify.** No RadioPost profiles beside Aether's: one list, kept by the modem, so the two can never disagree. RadioPost (1) shows which Aether profile is active; (2) switches it from its own window, through Aether; (3) lists the gateways this radio's modem and bandwidth can reach, from RPMI's `hello` (RP-4 §3) and the channels in the gateway list (RP-5 §4.4); (4) chooses what to fetch by time on the air at the rate the modem last reported (RP-4 §5), so a fast link fetches everything and a slow one only small messages, with no setting per radio. First, and now: a guide section on setting up several radios with one install. With other modems, the client keeps a short list of modem addresses. |
| Status | Proposed. |
| Author decision | **Decided** (2026-11-11): switching between radios only, one at a time. Two radios at once is deferred (§14). |
| Notes | Recommend against a profiles request in RPMI: other modems have none (VARA users keep a copy per radio too), so every adapter would carry it for one modem's sake. A profile switch changes the whole station: on an Aether shared with another program, such as a gateway, it moves that program's radio too, so it should be refused while another program's session or call is up (§13, G8). Whether Aether refuses it is to be checked. The restart drops RadioPost's claim; the gateway reconnects by itself, and the client should too. |

---

## 6. Sessions and radio operation

### W14. Choosing a gateway tunes the radio

Pick a gateway from the list, and the radio goes to its channel.

| | |
|---|---|
| Requested by | The plan: concept §10, first version. Requesters: 0. |
| Component | Client, adapters. |
| Compatibility | **Optional protocol addition** to RP-4: a request to read and set the dial, with a `caps` member; an adapter that cannot answers `unsupported` (RP-4 §2). Aether can; VARA's published commands cannot (RP-4 §9.2). Driving Aether's own API beside RPMI works today, but only with Aether, which is what RP-4 exists to avoid. |
| Existing coverage | Planned; not built. The panel says which channel to tune to. |
| Dependencies | None. Shares its addition with W40 (§12, T1). |
| Effort | Medium: RP-4 and its vectors, the Aether adapter, the client. |
| Priority | **Medium.** Every call starts with tuning, and it removes a common mistake. |
| Recommendation | Accept, with one RP-4 addition designed for W14 and W40 together. |
| Status | Planned. |
| Author decision | None yet. |
| Notes | **Needs specification review** on a shared modem: RP-4 §1.1 gives each host its callsigns, answering and links, and says nothing of what belongs to the whole station, like the dial (§13, G8). |

### W15. Keep listening for stations

Leave the client answering station-to-station calls, not just the first.

| | |
|---|---|
| Requested by | The editor, from [the on-air tests guide](../user/on-air-tests.md), §7: *Listen* answers one call, then stops. Requesters: 0. |
| Component | Client. |
| Compatibility | **Fits today**: `listen` (RP-4 §4) stays on. |
| Existing coverage | New. |
| Dependencies | None. |
| Effort | Small. |
| Priority | **Medium.** Station-to-station tests are what testers do first. |
| Recommendation | Accept. |
| Status | Proposed. |
| Author decision | None yet. |
| Notes | A station answering with nobody at it is under automatic control (§97.221). Aether's own documents read every Aether signal as wider than 500 Hz, which confines it to the §97.221(b) segments, and Aether's gate enforces that reading (RP-4 §8). That reading is Aether's; this list has not checked it. |

### W16. Scheduled connections

Call a gateway at set times to collect and send mail, unattended.

| | |
|---|---|
| Requested by | The plan: concept §10, later. Requesters: 0. |
| Component | Client. |
| Compatibility | **Fits today**: ordinary calls (RP-3). |
| Existing coverage | Planned; not built. |
| Dependencies | W40 with any modem but Aether. |
| Effort | Medium: the schedule, retries, a report of each call. |
| Priority | **Low.** |
| Recommendation | Defer until users ask. |
| Status | Planned. |
| Author decision | None yet. |
| Notes | An unattended call is automatic control: allowed in the US only in the segments of §97.221(b) (RP-7 §5.1 has them). Aether refuses elsewhere; with another modem the client must check (RP-4 §8). |

### W17. Gateways ranked by propagation

The gateway list puts first the gateways the user can probably reach now, not just the nearest.

| | |
|---|---|
| Requested by | The plan: concept §9 and §10, later. Requesters: 0. |
| Component | Client; for measurements, gateways and post offices too. |
| Compatibility | **Fits today** for a prediction: grid and channels are in the list (RP-5 §4.4). Ranking by what gateways measured is an **optional protocol addition** to RP-5 §4.4 or a new public document. |
| Existing coverage | Planned; ranking by distance is built. |
| Dependencies | None for a prediction. |
| Effort | Medium for a prediction; large for measurements. |
| Priority | **Low.** |
| Recommendation | **Modify**: a prediction only; defer measurements until a prediction proves too poor. |
| Status | Planned. |
| Author decision | None yet. |

### W18. Mail coming down compressed

Mail from the Internet takes less air time on its way down.

| | |
|---|---|
| Requested by | The editor, from reading the post office. Requesters: 0. |
| Component | Post office. |
| Compatibility | **Fits today**: every station must take DEFLATE (RP-2 §4, code 1), and gateways pass the post office's air form on as it is (RP-3 §7). |
| Existing coverage | New. The post office makes every air form uncompressed (code 0), as the reference that makes the vectors does, to keep them byte for byte; the client already compresses what it sends. The concept counts on compression (concept §5.1). |
| Dependencies | None. |
| Effort | Small. |
| Priority | **High.** Air time on every message a pilot user picks up. |
| Recommendation | Accept. |
| Status | Proposed. |
| Author decision | None yet. |
| Notes | RP-5 §5.3 does not say which compression a post office may use, and it cannot see the caller's `caps`, so only codes 0 and 1 are safe (§13, G2). |

### W19. Shorter short messages: a dictionary

Short messages, most of HF traffic, take a fifth less air time or better.

| | |
|---|---|
| Requested by | The plan: D17, RP-2 §4, design §10. Requesters: 0. |
| Component | Client, gateway, post office. |
| Compatibility | **Optional protocol addition** while optional: code 3 is reserved and negotiated (RP-3 §5.1, `caps`). RP-2 §4 says it then *becomes a requirement*, a **breaking protocol change** once Stable (§13, G7). |
| Existing coverage | Planned; the corpus is owed. |
| Dependencies | W18 for any gain on mail coming down. |
| Effort | Medium: write the corpus, train, measure, publish. |
| Priority | **Low.** A measurement before it is a feature. |
| Recommendation | Defer until the corpus is measured. |
| Status | Planned. |
| Author decision | **Decided** (2026-11-09, D17, CLAUDE.md): trained only on a published corpus, adopted only if it beats DEFLATE by a measured margin. |
| Notes | The post office makes one air form for every caller (RP-5 §5.3), so it can use code 3 only if every caller takes it (the requirement), or by keeping a form per code, which breaks resuming downloads across gateways (RP-3 §7). |

### W23. A phone version

Read, write and send by radio from a phone.

| | |
|---|---|
| Requested by | The plan: concept §10, later; design §6; Phase 5. Requesters: 0. |
| Component | Client. |
| Compatibility | **Fits today** over the Internet (RP-5 §6; mail programs already read the mailbox, concept §5.4), and by radio if the adapter runs on the phone and speaks the modem's own interface. RPMI from the phone to the computer at the radio is an **optional protocol addition** to RP-4 (§13, G4). |
| Existing coverage | Planned, Phase 5. |
| Dependencies | A modem a phone can drive; Aether's own plan for one is on its back burner. |
| Effort | Large. |
| Priority | **Low.** |
| Recommendation | Defer until such a modem exists. |
| Status | Planned. |
| Author decision | None yet. |

---

## 7. Gateway and post office operation

### W24. Block a caller by address or callsign

The gateway's operator refuses one account, or one station, that misuses the gateway.

| | |
|---|---|
| Requested by | The plan: concept §9. Requesters: 0. |
| Component | Gateway. |
| Compatibility | **Fits today**: `ERROR` `refused-by-policy` after `HELLO`, with a text (RP-3 §5.9, RP-0 §9.3). |
| Existing coverage | Partly built: the operator can block post offices from the command line (RP-1 §7.3); not addresses or callsigns, and nothing in the panel. |
| Dependencies | None. |
| Effort | Small. |
| Priority | **Medium.** Operators need it before a gateway is widely known; they stop violating traffic once they know of it (RP-6 §5.2). |
| Recommendation | Accept, in the panel too. |
| Status | Planned. |
| Author decision | None yet. |

### W26. One gateway, several modems

One gateway answers callers whatever modem they run: Aether HF and Mercury on the same channel, or
HF and VHF packet, each modem on a radio of its own or sharing one.

| | |
|---|---|
| Requested by | The plan: design §5.2 (*Modem host*); the county EOC of concept §2. The author (KK4ODA), 2026-11-11: Aether HF and Mercury on one gateway. Requesters: 1. |
| Component | Gateway. |
| Compatibility | **Fits today.** The gateway holds one RPMI connection per adapter (RP-4 §2), and its record lists every channel with its modem (RP-5 §4.4): the same dial twice, once per modem, is allowed. Two modems on one radio need no protocol either: the gateway turns `listen` off on the others while one has a call or a link (RP-4 §4), and each modem hears the other's signal as busy (RP-4 §5). Listing a Mercury channel needs a modem name RP-5 §4.4 does not have: an **optional protocol addition** (§13, G11). |
| Existing coverage | Planned; one modem per gateway today. |
| Dependencies | W20 for Mercury; W22 for packet; W40, since Mercury checks no band rules; G11. |
| Effort | Medium for a modem per radio. Large for two modems on one radio: shared audio and keying, the gateway holding one modem quiet while the other works, a guide, and tests with two real modems. |
| Priority | **Medium.** A gateway that serves both modems' users doubles who can reach it; it is not needed for the pilot. |
| Recommendation | Accept, as one capability configured per radio: modems on separate radios run side by side, and modems sharing a radio take turns, the gateway keeping the others from answering while one has a call or link. |
| Status | Planned. |
| Author decision | None yet. |
| Notes | On one radio, both modems hear the same audio, which the operating system can share; only one may transmit at a time, and the radio's keying must be shared too: through rigctld or FLRig, which Aether supports and which Mercury's Hamlib keying can reach, since only one program can hold a CAT port. Two callers at the same instant could each be answered once; the gateway aborts the later, and the two calls had already collided on the air. Aether's gate checks only Aether's transmissions. Design D19 says two modems cannot share one sound card: true for transmitting, not for listening. |

### W36. One mailbox, by radio and by mail program

Mail read in a mail program is not offered again by radio. Mail sent by radio shows in the mail
program's Sent folder. Mail between users of the same post office reaches them whichever way they
read, including from members with no licence who use only a mail program.

| | |
|---|---|
| Requested by | The plan: concept §3.1 (email-only members), §5.4; RP-5 §6.3, `radio_skips_read`. Requesters: 0. Includes W12. |
| Component | Post office. |
| Compatibility | **Fits today.** Email-only members hold no credential (RP-1 §1), and their mail to radio users is third-party traffic (RP-6 §5.1). |
| Existing coverage | Partly built: the radio's mailbox and the mail program's are separate ([the post office guide](../user/post-office.md), §11). Mail a mail-program user sends to another user here never reaches the radio. |
| Dependencies | None. |
| Effort | Large: JMAP both ways, and every path mail takes inside the post office. |
| Priority | **High.** Mail that never reaches the radio is mail silently lost to a radio user, and pilot users with the Internet will meet it at once. |
| Recommendation | Accept. Also fill the accept list from mail sent in a mail program, as from mail sent by radio (RP-5 §6.5 names only radio paths): concept §5.1 promises that writing to someone adds them, and that removes most of the need for W37. |
| Status | Planned. |
| Author decision | None yet. |

### W37. The accept list on a web page

Add or remove senders from any browser.

| | |
|---|---|
| Requested by | The plan: concept §5.1. Requesters: 0. |
| Component | Post office. |
| Compatibility | **Fits today**: the post office's own page over the same list (RP-5 §6.5, RP-6 §2.2). |
| Existing coverage | Planned; the client changes the list. |
| Dependencies | A login for the page. W36 covers the common case. |
| Effort | Medium: a page, and a login to keep safe. |
| Priority | **Low.** |
| Recommendation | Defer: the client and W36 cover most uses, and a web login is one more thing to secure. |
| Status | Planned. |
| Author decision | None yet. |

### W38. Backups and a backup mail server

A post office that loses its server loses no mail, and is rebuilt in hours.

| | |
|---|---|
| Requested by | The plan: concept §8.5; the pilot is to run with nightly off-site backups and a backup mail server (concept §12). Requesters: 0. |
| Component | Post office. |
| Compatibility | **Fits today**: one operator's servers; no protocol. |
| Existing coverage | Planned. radiopost.org's mail has been live since 2026-11-11 (CLAUDE.md), and the post office guide sets up one server with no word on backups. |
| Dependencies | None. |
| Effort | Medium. |
| Priority | **Critical** for the backups: the pilot's server now holds real mail, its users' accounts and the post office's key. The backup mail server is **Medium**. |
| Recommendation | Accept: backups now, and a restore tried once; the backup mail server after. |
| Status | Planned. |
| Author decision | None yet. |
| Notes | A backup mail server must refuse as the door does, or it sends bounces to forged senders later (D15, RP-6 §2.1), so it needs the accept lists (concept §13 left this open). Without one, senders retry for days (RFC 5321) and only time is lost. |

---

## 8. Offline and disconnected operation

The author decided the order (CLAUDE.md, *Decided by the author*): copies placed ahead, then the
gateway's queue, then forwarding by radio.

### W27. Copies placed ahead

Mail waiting for a user is copied to their usual gateways, so it can be picked up while the
Internet or the post office is down. The user names those gateways in the client.

| | |
|---|---|
| Requested by | The plan: concept §6.1 and §10, D12, Phase 3. Requesters: 0. Includes W9. |
| Component | Post office, gateway; the client names the gateways. |
| Compatibility | **Fits today**: RP-5 §5.8 and §6.3, the flag `copy` (RP-3 §5.2), serials (RP-3 §6.2). |
| Existing coverage | Specified, not built. The setting `usual_gateways` can be changed only from the command line. |
| Dependencies | Phase 2. |
| Effort | Large: the long poll, the gateway's store, drops and expiry, the client's setting. |
| Priority | **High**: first of Phase 3. |
| Recommendation | Accept. |
| Status | Planned. |
| Author decision | **Decided** (CLAUDE.md): first of the Internet-down features. |

### W28. The gateway's queue while offline, and its reports

A gateway that has lost the Internet still checks callers, takes their mail and serves its copies,
and reports everything once it is back.

| | |
|---|---|
| Requested by | The plan: concept §6.2, Phase 3. Requesters: 0. |
| Component | Gateway, post office. |
| Compatibility | **Fits today**: RP-3 §9 and `online` (RP-3 §5.2), keys and revocation lists kept (RP-1 §7.3), `POST /reports` (RP-5 §5.6). |
| Existing coverage | Specified, not built: the gateway works only online. |
| Dependencies | W27. |
| Effort | Large. |
| Priority | **High**: second of Phase 3. |
| Recommendation | Accept, shipped with W29. |
| Status | Planned. |
| Author decision | **Decided** (CLAUDE.md): second, after copies placed ahead. |

### W29. The sender's client follows queued mail

The user sees mail as queued at a gateway; the client offers it again until a post office has it,
and says so if it may have been lost.

| | |
|---|---|
| Requested by | The plan: D25. Requesters: 0. |
| Component | Client. |
| Compatibility | **Fits today**: RP-7 §3.6. |
| Existing coverage | Specified, not built. |
| Dependencies | W28. |
| Effort | Medium. |
| Priority | **High.** Without it, a message a cut-off gateway drops is lost with nobody told (D25). |
| Recommendation | Accept, and ship with W28, never after it. |
| Status | Planned. |
| Author decision | None yet. |

### W30. Forwarding by radio

Mail leaves an area that has lost the Internet, and comes back in, by radio between gateways.

| | |
|---|---|
| Requested by | The plan: concept §6.3; D18, D21 to D23, D26; Phase 4. Requesters: 0. |
| Component | Gateway, post office. |
| Compatibility | **Fits today**: RP-7, with its additions to RP-3 and RP-5 already made. |
| Existing coverage | Specified, not built. |
| Dependencies | W27, W28; W26 when a partner is on another radio; W40 for modems other than Aether; W41 outside the US. |
| Effort | Large. |
| Priority | **Medium**: Phase 4. |
| Recommendation | Accept. |
| Status | Planned. |
| Author decision | **Decided** (CLAUDE.md): after the gateway's queue. |

### W31. Local delivery at a cut-off gateway

Neighbours who use the same cut-off gateway (a shelter and the EOC) exchange mail at once.

| | |
|---|---|
| Requested by | The plan: D24. Requesters: 0. |
| Component | Gateway; the client asks by content ID (RP-3 §5.6, `ids`). |
| Compatibility | **Fits today**: RP-7 §4.4, reported with `local_pickups` (RP-5 §5.6). |
| Existing coverage | Specified, not built. Specified in RP-7, so listed in Phase 4. |
| Dependencies | W27, W28. It needs no partners and no forwarding. |
| Effort | Medium. |
| Priority | **Medium.** In an outage, much of the mail that matters is between neighbours (D24). |
| Recommendation | **Modify**: build it right after W28, in Phase 3, not with W30. It needs only the copies (which say who the local users are) and the offline reports. This keeps the author's order: forwarding still comes after the queue. |
| Status | Planned. |
| Author decision | None yet. |

---

## 9. Forms and applications

### W32. Forms

Fill in ICS-213 and other public forms. The recipient sees the form; an email reader gets a
readable copy.

| | |
|---|---|
| Requested by | The plan: concept §10, later; design §9, Phase 5; RP-2 §2 reserves key 13 for forms. Requesters: 0. |
| Component | Client; a published form format. |
| Compatibility | Recommended: an **application-layer capability**, the form's data as a file of a published type in `files`, with the sender's client writing the readable text in `body`, so any client and any email reader shows it; nothing below changes. As reserved: an **optional protocol addition** defining key 13 (RP-2 §2), with RP-6 saying how a form becomes email. |
| Existing coverage | Planned, Phase 5; key 13 reserved. |
| Dependencies | None. |
| Effort | Large: a template format, a set of forms, an editor and a viewer. |
| Priority | **Medium**; likely to rise, since emergency groups ask for forms first. |
| Recommendation | **Modify**: application layer first. Interoperability needs a published form format either way, but as a file it asks nothing of RP-2 or RP-6. Use key 13 only if a measured air-time saving pays for a specification. |
| Status | Planned. |
| Author decision | No compatibility with Winlink's programs (CLAUDE.md, decided): templates from FEMA's public forms in an open format (concept §10), not Winlink's. |

### W33. Position reports

Send where a station is, for a map or a net.

| | |
|---|---|
| Requested by | The plan: concept §10, later; RP-4 §6 reserves broadcasts for it. Requesters: 0. |
| Component | Client; a service that collects them. |
| Compatibility | **Application-layer capability** as a message to a position service, at a session each. As a broadcast to everyone listening, an **optional protocol addition** (a new specification) carried only by modems with `caps.broadcast` (RP-4 §6). |
| Existing coverage | Planned; not built. |
| Dependencies | None. Shares a broadcast format with W34 (§12, T4). |
| Effort | Medium. |
| Priority | **Low.** |
| Recommendation | Defer; application layer first when wanted. |
| Status | Planned. |
| Author decision | None yet. |
| Notes | Whether a station under automatic control may send such a broadcast must be checked against the rules before a broadcast is designed. |

### W34. Bulletins

Read a net's or an agency's bulletins.

| | |
|---|---|
| Requested by | The plan: concept §10, later; RP-4 §6. Requesters: 0. |
| Component | A list service; the client. |
| Compatibility | **Application-layer capability** as mail from a list address: every reader downloads a copy. As a broadcast: one transmission for everyone, an **optional protocol addition**, and no acknowledgement, so a missed bulletin still needs the mail copy. |
| Existing coverage | Planned; not built. |
| Dependencies | None for mail. |
| Effort | Medium for mail; large for broadcasts. |
| Priority | **Low.** |
| Recommendation | Defer; mail first. Broadcasts only if a busy net shows the air time matters, and then one format with W33. |
| Status | Planned. |
| Author decision | None yet. |

### W35. Services that answer by mail

Write to an address for a catalogue, a weather report or a blank form, and get the answer as mail.

| | |
|---|---|
| Requested by | The plan: concept §10, later (catalogue requests). Requesters: 0. |
| Component | Application layer: a service at a post office or anywhere on the Internet. |
| Compatibility | **Application-layer capability**: ordinary mail both ways. Writing to it puts it on the accept list (RP-5 §6.5). |
| Existing coverage | Planned; nothing needed from RadioPost. |
| Dependencies | None. |
| Effort | Small for each service. |
| Priority | **Low.** |
| Recommendation | Accept as the application layer's: anyone may run one. |
| Status | Planned. |
| Author decision | None yet. |
| Notes | An answer arriving as ordinary email is third-party mail (RP-6 §5.1), held back from stations in countries without an arrangement. |

---

## 10. Adapters and interoperability

### W20. An adapter for VARA-style modems: VARA HF, VARA FM and Mercury

Use RadioPost with VARA, which many stations already run, or with Mercury, Rhizomatica's open
HF modem, as a client or a gateway.

| | |
|---|---|
| Requested by | The plan: concept §7, design §5.4, Phase 5 (VARA). The author (KK4ODA), 2026-11-11 (Mercury). Requesters: 1. |
| Component | Adapter. |
| Compatibility | **Fits today.** Mercury offers a VARA-compatible TNC interface (its `docs/TNC.md`: commands on port 8300, data on 8301), so RP-4 §9.2's mapping serves both; a note in RP-4 §9 on where Mercury differs is informative, a PATCH. Neither can be shared: a host holds the single `MYCALL` list. Neither checks band rules (RP-4 §8). Mercury's `BUFFER 0` means the other station has acknowledged everything, so its adapter can report `delivered`; VARA's means only sent. Naming Mercury in a gateway's record needs G11. |
| Existing coverage | Planned for VARA, Phase 5. Mercury is new to the plan. |
| Dependencies | W40 for an unattended gateway; G11 to list Mercury channels. |
| Effort | Medium: one adapter, with each modem's differences. |
| Priority | **Medium.** The author asks for Mercury, and the two modems reach many users. |
| Recommendation | **Modify**: one adapter for both, built and tested against Mercury first. Mercury is open source and runs on Linux, so the adapter and the tester's A-01 to A-04 can run against a real modem in CI; VARA is proprietary and is never run by the project, so it follows from its published commands. Whether Mercury joins Phase 5's adapters, ahead of ARDOP, is the author's call. |
| Status | Planned. |
| Author decision | VARA is a black box (CLAUDE.md): only its published command list is used. |
| Notes | Mercury's air interface is published (FreeDV and codec2 OFDM modes, documented in its repository), so an open monitor (W42) can read sessions over it, which it cannot over VARA. Mercury is GPL-3.0; the adapter talks to it over TCP, as the post office talks to Stalwart, so RadioPost's licences are unaffected. |

### W21. An ARDOP adapter

Use RadioPost over ARDOP.

| | |
|---|---|
| Requested by | The plan: concept §7, design §5.4, Phase 5. Requesters: 0. |
| Component | Adapter. |
| Compatibility | **Needs specification review**: RP-4 §9.3's mapping is to be checked against ARDOP's own specification before an adapter is written (design §10). |
| Existing coverage | Planned, Phase 5. |
| Dependencies | That check; W40 for unattended use. |
| Effort | Medium. |
| Priority | **Low.** |
| Recommendation | Defer until the check is done. |
| Status | Planned. |
| Author decision | None yet. |

### W22. An AX.25 packet adapter

Serve local users on VHF and UHF packet through Direwolf or a TNC, as the county EOC does in the
concept (N2CALL-5).

| | |
|---|---|
| Requested by | The plan: concept §2 and §7, design §5.4, Phase 5. Requesters: 0. |
| Component | Adapter. |
| Compatibility | **Fits today** through AGWPE (RP-4 §9.4). Whether it can be shared depends on the TNC software (RP-4 §9.4). |
| Existing coverage | Planned, Phase 5. |
| Dependencies | W26 to run beside HF; W40 for unattended use. |
| Effort | Medium. |
| Priority | **Low.** |
| Recommendation | Accept. |
| Status | Planned. |
| Author decision | None yet. |

### W43. A public test service

Someone writing their own client, gateway or adapter tests it against RadioPost's tester over the
Internet, and on the air against a test gateway.

| | |
|---|---|
| Requested by | The plan: [the conformance tester](01-conformance.md), §7, not settled. Requesters: 0. |
| Component | Tools. |
| Compatibility | **Fits today**: the tester exists (`core/tester`); the vectors are published (RP-0 §8.6). |
| Existing coverage | Not settled. |
| Dependencies | Real keys for an on-air test gateway: the published test keys never go on the air (the tester's §2.3). |
| Effort | Medium. |
| Priority | **Low**; rises with the first independent implementation, which every specification needs before it becomes Candidate (RP-0 §8.1). |
| Recommendation | Defer until an independent implementer appears. |
| Status | Proposed. |
| Author decision | None yet. |

---

## 11. Operator safety and regulatory compliance

### W40. The gateway checks the band rules itself

With a modem that checks nothing (VARA, ARDOP, packet), an unattended gateway still keeps to the
automatic-control segments.

| | |
|---|---|
| Requested by | The plan: design §5.2 (*Rules check*), concept §7; RP-4 §8 requires it of the host or its operator. Requesters: 0. |
| Component | Gateway; the client too, for W16. |
| Compatibility | **Fits today** with the dial taken from the gateway's settings or registered channels (RP-5 §4.4), judged against its country's table (RP-7 §5.1), or read by the gateway itself from the rigctld or FLRig its modems key through (W26). Reading the dial from the modem is an **optional protocol addition** to RP-4: RPMI carries no frequency (§13, G3). |
| Existing coverage | Partly built: a setting says the operator checks ([the gateway guide](../user/gateway.md), §2); the gateway checks nothing itself. |
| Dependencies | W14's RP-4 addition to read the dial (§12, T1); W41 outside the US. |
| Effort | Medium. |
| Priority | **Medium**: needed before W20 to W22 run unattended. |
| Recommendation | Accept. |
| Status | Planned. |
| Author decision | None yet. |

### W41. Rules tables for other countries

A gateway outside the US hands over third-party mail, and forwards unattended, by its own
country's rules.

| | |
|---|---|
| Requested by | The plan: design §10; RP-6 §5.1, RP-7 §5.1, §8. Requesters: 0. |
| Component | Gateway; the specifications. |
| Compatibility | **Optional protocol addition**: a country's table in RP-6 §5.1 or RP-7 §5.1 changes nothing for gateways elsewhere. Each is read from that country's rules, with the date. |
| Existing coverage | The United States only; elsewhere the operator enters prefixes and segments (RP-6 §5.1, RP-7 §5.1). |
| Dependencies | Someone reading each country's rules. |
| Effort | Medium for each country. |
| Priority | **Low** until a gateway outside the US joins. |
| Recommendation | Defer until then. |
| Status | Planned. |
| Author decision | None yet. |

### W42. A monitor that decodes sessions off the air

Anyone can record a RadioPost session and read every message in it with an open tool.

| | |
|---|---|
| Requested by | The plan: concept §11, *Monitoring*. Requesters: 0. |
| Component | Application layer: a tool. |
| Compatibility | **Fits today**: everything on the air is published (RP-0 §7; RP-2 §4; RP-3 §2, §4), and so is Aether's air interface. |
| Existing coverage | Planned; not built. |
| Dependencies | Aether giving a session's bytes from a recording. |
| Effort | Medium. |
| Priority | **Medium.** The concept's answer to the "obscured meaning" complaint, and a debugging tool for every implementer. |
| Recommendation | Accept. |
| Status | Planned. |
| Author decision | None yet. |
| Notes | Over VARA it cannot exist: VARA's air interface is not published. |

---

## 12. Threads that run through several wishes

Where wishes share a need, building it once serves them all. None of these is a wish of its own.

| # | Thread | Wishes | What to do once |
|---|---|---|---|
| T1 | The modem's dial | W14, W40, W16, W26 | One optional RP-4 addition to read and set the dial, with its rule for a shared modem (G3, G8). Where the modems key through rigctld or FLRig, a gateway can read the dial there itself, with no addition. |
| T8 | Modem names | W20, W26, W44 | One registry of modem names, and what a client does with one it does not know (G11): gateway lists, the filter by modem, a new adapter. |
| T2 | Credentials that never lapse unattended | W8 | One renewal rule for clients and gateways, by any open path. |
| T3 | The post office's air forms | W18, W19 | One rule for which compressions a post office uses (G2), settled before the dictionary. |
| T4 | Broadcasts | W33, W34 | One broadcast format, only if one of them shows a need mail cannot meet. |
| T5 | Unattended operation | W15, W16, W27, W28, W30 | The rules check (W40) for every modem but Aether, and an honest note of what the rules allow (§1). |
| T6 | One mailbox | W36 | Absorbs W12, and most of W37 once mail sent from a mail program fills the accept list. |
| T7 | The rate the modem reports | W3, W44 | One estimate of time on the air (RP-4 §5, `quality`), used both to show how long the outbox will take and to choose what to fetch. |

---

## 13. Gaps in the specifications and the design

Each is noted under its wish and kept here for a separate discussion. Nothing is changed in this
document.

| # | Where | What is missing | What it costs | A possible approach | Wish |
|---|---|---|---|---|---|
| G1 | RP-2 §6 | A client SHOULD let the user ask for a trimmed message's full text; nothing carries the request (RP-3 §5.6, RP-5 §5.3). | A SHOULD no client can meet; implementers may invent private ways. | Say instead: show the message `reply_to` names when the client holds it. | W4 |
| G2 | RP-5 §5.3 | Which compression the post office's air form may use. It never sees the caller's `caps` (RP-2 §4 has the sender choose one both ends take). | A post office using code 2 would serve mail some clients cannot read; ours serves everything uncompressed. | Code 0 or 1 only, 1 when smaller. | W18, W19 |
| G3 | RP-4 | Any frequency. RP-4 §8 has the host check the band rules when the modem does not, with no way to learn the dial. | Gateways on other modems trust a configured dial; the client cannot tune. | An optional dial capability (T1). | W14, W40 |
| G4 | RP-4 §2 | A transport off the machine: loopback or a Unix socket only, and no authentication. | No common way for a phone to drive a modem in another box. | Leave it until a phone needs it. | W23 |
| G5 | Concept §6.2 | *Delivered to the recipient's provider* is promised; nothing reports it (RP-5, RP-6). | Users are promised what no program can show. | Drop it from the concept, or an optional client endpoint in RP-5 §6. | W6 |
| G6 | Design §5.1 | Says the client keeps mail as `.eml`; the client keeps the canonical encoding. | Design and program disagree. | Export `.eml`, and say so in the design. | W5 |
| G7 | RP-2 §4 | Dictionary 1 "becomes a requirement" once adopted. | A change others cannot ignore, where a negotiated option would not be one. | Decide with the corpus's numbers, with G2. | W19 |
| G8 | RP-4 §1.1 | What belongs to the whole station on a shared modem, such as the dial, or a whole Aether profile (RP-4 §9.1 leaves the bandwidth open in the same way). | One host can move every other host's station, even to another radio. | Refuse a station-wide change while another host's link or call is up. | W14, W44 |
| G9 | RP-3 §8 | Whether a second session may follow on the same link. | A client with several accounts pays a call for each. | Leave it, or allow it when both sides say so. | W7 |
| G10 | RP-1 §6.3 | Step 2 requires the statement's `operator` to equal the credential's, yet `operator` is the callsign "as on the air" (RP-1 §6.1, RP-3 §5.1), and RP-6 §5.1 and D31 read where a station is from a prefix or suffix on it (`VE3/KK4ODA`). | A user abroad who signs with the host country's prefix is refused unless enrolled again for it; one who keeps the plain callsign is taken for a station at home, and the third-party check judges the wrong country. | Compare the base callsign, and let the statement carry the prefix and suffix: a change every verifier must make, cheapest while the specifications are Drafts. | W44 |
| G11 | RP-5 §4.4 | Its `modem` values (`aether`, `vara-hf`, `vara-fm`, `ardop`, `ax25`, `tcp`) are a list with no registry, no way to add one, and no rule for a value a client does not know. | A gateway offering Mercury has no agreed name for it, and clients may treat an unknown name differently. | A registry of modem names in RP-0 §9, `mercury` its first new entry, and a client ignoring a channel whose modem it does not know. | W20, W26, W44 |

---

## 14. Consolidated, deferred and rejected

Kept so that a wish refused once is not argued again from the start, and an idea not built now is
not lost. A wish is reconsidered on new evidence, not on being asked again by the same person.

| Idea | Requested by | Why | The alternative | Decided | Reconsider if |
|---|---|---|---|---|---|
| W9. Usual gateways set in the panel | The plan | Consolidated into W27: the setting does nothing until copies are placed ahead. | Built as part of W27. | 2026-11-11, the editor's proposal | — |
| W12. Email-only members | The plan | Consolidated into W36: what they need is its mail path. | Built as part of W36. | 2026-11-11, the editor's proposal | — |
| W25. The gateway renews its own credential | The plan | Consolidated into W8: one renewal rule for every credential. | Built as part of W8. | 2026-11-11, the editor's proposal | — |
| W39. A domain served by several post offices | The plan: concept §12, a later option | Not a goal for now (concept §1). A breaking protocol change: RP-1 §7.1 allows one API record per domain, and serials (RP-3 §6.2) are one post office's numbering. | Backups and a quick rebuild (W38); a user who owns their domain can move it. | The concept, 2026-11-09 | A group needs addresses that outlive its post office, and W38 is not enough. |
| Two radios at once in one install (from W44) | The author | Needs Aether to run a modem per radio under one app, which is Aether's work; RadioPost would then hold a connection to each (RP-4 §2) and listen on both. For a gateway, W26. | Switch between radios (W44). | **The author**, 2026-11-11: switching only for now | Operators need HF and VHF, or two bands, live at the same time on one computer. |
| Several accounts in one session (from W7) | The editor | A breaking change to RP-1 and RP-3 to save one call. | A call per account (W7). | 2026-11-11, recommended against; not decided | Emergency operators show the extra call matters. |
| Work with Winlink's programs, or speak B2F | The concept | A client and protocol of RadioPost's own. | — | **The author**, 2026-11-09 ([CLAUDE.md](../../CLAUDE.md)) | Only by the author. |
| Encrypted or private mail on the air | The concept | Conflict: the rules forbid messages encoded to obscure their meaning (§97.113(a)(4)); RadioPost signs and never encrypts (RP-0 §5, §7). | Signatures for who sent what; encrypted email is refused at the door (RP-6 §2.1). | The concept and RP-0, 2026-11-09 | The rules change. |
| Email for people who are not amateurs | The concept, §1 | RadioPost is not a public email provider. | Email-only members of a group (W36). | The concept, 2026-11-09 | — |
| A list of post offices kept by the project | D9, D32 | A gatekeeper, against the design's having no centre. | DNS, and each operator's own blocks (RP-1 §7.3). | **The author**, 2026-11-09 (D32) | Only by the author. |
| Gateways that announce what they reach | D23 | An announcement must be trusted; a false one draws an area's mail away. | Configured partners and published routes (RP-7 §6). | The design, 2026-11-09 | Configured partners prove unworkable in a real outage. |
| The gateway sending email itself | D16 | It cannot sign for the user's domain; the mail is filtered or refused. | Mail leaves from the sender's post office. | The design, 2026-11-09 | — |
| Taking mail and bouncing it later | D15 | Bounces go to forged senders. | Refusal at the SMTP door. | The design, 2026-11-09 | — |
| A dictionary trained on real mail | D17 | A dictionary is published and keeps its training text verbatim. | A corpus RadioPost writes and publishes (W19). | **The author**, 2026-11-09 (D17) | Only by the author. |
| A password sent over the air | The concept, §3.3 | Nothing secret goes over the air. | Signatures prove who is calling. | The concept, 2026-11-09 | — |

---

## Change log

| Version | Date | Change |
|---|---|---|
| 0.1.0 | 2026-11-10 | First draft: how a wish is recorded; seeded with what the concept and the design promise and have not built, and two things found in the programs (W1 to W43); the gaps found on the way (G1 to G7); what the design has already set aside. |
| 0.2.0 | 2026-11-11 | Rebuilt to the author's brief: verdicts as the brief names them, with *needs specification review*; priorities Critical, High, Medium, Low; a recommendation, a status and the author's decision for each wish; principles (§2) and shared threads (§12). W9 folded into W27, W12 into W36, W25 into W8; W39 deferred. W31 proposed for Phase 3, W32 for the application layer, W36 to fill the accept list from mail-program mail; W21 needs specification review. Gaps G8, G9; what each gap costs. Statements about the rules say whose reading they are. |
| 0.2.1 | 2026-11-11 | The author's first wish: W44, one install for several radios, recommended as RadioPost following Aether's profiles rather than keeping its own. Thread T7, the modem's rate; gap G10, a portable callsign against the credential's; G8 covers a profile switch. §5 renamed *Setup, accounts and configuration*. |
| 0.2.2 | 2026-11-11 | W44: the author decided on switching between radios only; two radios at once deferred (§14). |
| 0.2.3 | 2026-11-11 | The author's wish for Aether HF and Mercury on one gateway, merged: W26 becomes *one gateway, several modems* (on separate radios or sharing one), W20 *an adapter for VARA-style modems*, Mercury included, built against Mercury first; both Medium. W40 may read the dial from rigctld or FLRig; thread T8, modem names; gap G11. W38: radiopost.org's mail is live, still with no backups. W36 cites the post office guide's §11. |
| 0.2.4 | 2026-11-11 | W8 implemented: credentials renew themselves in their last month, the client's by Internet or radio and the gateway's own. |
