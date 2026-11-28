# RP-8: Alerts and reports

| | |
|---|---|
| **Status** | Draft |
| **Version** | 0.4.0 |
| **Updated** | 2026-11-28 |
| **Editor** | KK4ODA |
| **Licence** | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| **Depends on** | RP-0, RP-2, RP-4, RP-5 |

How one station tells another, or a group of stations, that it needs them now: a line of text that
the other station's program shows as an alarm, with a way to answer. An alert is a **broadcast**
(RP-4 §6): it needs no call and no session, so it reaches a station over a path too weak to connect
on, and it can reach several stations at once.

Only stations that share a **keyword** can alert each other. The keyword makes a key, and every
alert carries a short **tag** that only that key can make. Nothing is encrypted: anyone listening
can read every alert, and only the stations that know the keyword can make one that their programs
will sound for (RP-0 §7).

A **report** goes the same way: a form (RP-2 §8), such as a Position Report or a Storm Report, from
one station to its circle, for the circle's map. Only the circle's stations show it, and nobody
answers it (§5).

---

## 1. Model

* A **circle** is the stations whose operators typed the same keyword into their programs: a
  family, a net, a team. The keyword gives the circle's key (§2). An operator may be in several
  circles; each is a keyword.
* An **alert** is from one station's operator, to one station or to the whole circle. It carries a
  line of text, the time, and a tag made with the circle's key (§3).
* The station alerted answers it (§5): **heard** at once, by itself; **seen** when its operator
  acknowledges it; and a **reply**, a line of text, if the operator writes one.
* A **report** is a form from one station's operator, to the whole circle or to one station: its
  values only, as in a message (RP-2 §8.2), with the time and a tag. It is not answered.
* A station that hears a broadcast checks it (§4). A frame that is not one of these, that names a
  circle the station does not know, or whose tag does not check, is dropped without a sound.

Alerts and reports are for the moment: a few minutes, until the next. Anything that must arrive
later, or must be kept, goes as mail (RP-2).

---

## 2. The circle's key

The keyword is first **normalised**, so that two operators who type it differently still make the
same key:

1. Unicode NFC (RP-0 §4.3).
2. The ASCII capital letters `A` to `Z` become lower case. No other character changes.
3. Every run of spaces (U+0020) and tabs (U+0009) becomes one space, and spaces at either end are
   removed.

The circle's **key** is PBKDF2 with HMAC-SHA256 (RFC 8018 §5.2) of the normalised keyword's UTF-8
bytes, with the ASCII bytes of the context `RadioPost-circle-v1` as the salt, 600 000 iterations,
and 32 bytes of output. A program works the key out once, when the keyword is typed, and keeps the
key. It MAY keep the normalised keyword too, so that its operator can tell it to someone who joins
the circle later. It then shows the keyword only when the operator asks, and MUST NOT send it
anywhere: not on the air, to a modem, or to a post office.

The circle's **ID** is the first 4 bytes of HMAC-SHA256 under the key of the context
`RadioPost-circle-id-v1` and one zero byte (RP-0 §5, with no bytes after the zero). Every alert
names its circle by it.

A program shows the ID as the circle's **check**: its 8 lowercase hexadecimal digits in two groups
of four (`9853 7abf`). Operators setting up a circle read their checks to each other: the same check
means the same keyword.

**The keyword.** Anyone who hears one alert can try keywords against its tag, offline, as fast as
they can compute the key. The iterations slow each try; only a keyword nobody would guess stops
them. A program MUST refuse a keyword of fewer than 8 characters once normalised, and SHOULD
offer to make one: four or more random words or syllables. A keyword is told to the other members
by voice or in person, never on the air.

Test vectors: `vectors/alerts.json`, `circles` and `keywords`.

---

## 3. The frame

An alert, each answer to it, and a report is one CBOR map (RP-0 §4.1), sent as a broadcast's
`data` (RP-4 §6). Its key 0 names the kind of broadcast, from RP-0 §9.5; RP-8 defines five:

| `t` | Kind | What it is |
|---|---|---|
| 1 | alert | An alert (§1). |
| 2 | heard | The station alerted has heard the alert (§5). |
| 3 | seen | Its operator has seen it (§5). |
| 4 | reply | Its operator's line of text in answer (§5). |
| 5 | report | A form, to the circle or to one station (§1). |

| Key | Name | Type | Kinds | Meaning |
|---|---|---|---|---|
| 0 | `t` | uint | all | The kind, above. |
| 1 | `circle` | bstr (4) | all | The circle's ID (§2). |
| 2 | `from` | tstr | all | The sender's callsign (RP-0 §6.1), in upper case. |
| 3 | `to` | tstr | all; an alert and a report may leave it out | The station the frame is for. An alert or a report without it is for every station in the circle. An answer is for the alert's `from`. |
| 4 | `time` | uint | all | When it was sent, Unix seconds (RP-0 §4.4). |
| 5 | `text` | tstr | alert and reply only | One line: 1 to 100 bytes of UTF-8, in NFC, with no control characters (U+0000 to U+001F, U+007F to U+009F). |
| 6 | `re` | bstr (8) | heard, seen and reply only | The tag of the alert it answers. |
| 7 | `tag` | bstr (8) | all | The tag, below. |
| 8 | `form` | map | report only | The form: RP-2 §8.2's map, key 1 the template's fingerprint (8 bytes) and key 2 its values by field number, 1 to 255, at least one. |

`text` is present in an alert and a reply and absent from the others, `re` is present in the
three answers and absent from an alert and a report, and `form` is present in a report and absent
from the others. A receiver ignores keys it does not know (RP-0 §4.1); the tag covers them too.
Keys 0–63 are the specifications' (RP-0 §4.1). A frame is at most 512 bytes.

Whether a report's values fill its template (RP-2 §8.2) is for a station that holds the template to
check, as for a form in a message. A station that does not hold it keeps the report, and shows its
values by number (RP-2 §8.4).

The **tag** is the first 8 bytes of HMAC-SHA256 under the circle's key of the context
`RadioPost-alert-v1`, one zero byte, and the map's deterministic encoding without key 7
(RP-0 §5). The frame is then the map with key 7 added, deterministically encoded.

The tag names the alert too: an answer's `re` is it, and a station that hears the same tag twice has
heard the same frame twice (§4).

Facundo (KK4ODA) alerts Ana (N0CALL) in his family's circle; 72 bytes:

```
{0: 1, 1: h'98537abf', 2: "KK4ODA", 3: "N0CALL", 4: 1794667200,
 5: "Please call me on 146.52 now", 7: h'775a35a77a50842a'}
```

Her station's `heard`, 51 bytes:

```
{0: 2, 1: h'98537abf', 2: "N0CALL", 3: "KK4ODA", 4: 1794667214,
 6: h'775a35a77a50842a', 7: h'55e5755012a5e48c'}
```

**Air time.** Aether carries a broadcast as a datagram (RP-4 §9.1) at its rung 1, in the tone frames
both its bandwidths hear: 33 bytes of the datagram a frame of 5.36 s, after Aether's own 8 (the
station's callsign and the frame's type). So the alert above is three frames, 16.1 s on the air,
and the `heard` two, 10.7 s, at 500 Hz and at 2300 Hz alike (measured with Aether's model, its
`datagram.split`). A longer line costs a frame for each 33 bytes more: the longest, 100 bytes,
takes five frames, 26.8 s.

Facundo (KK4ODA) reports his position to Shelter 3's circle with a Position Report, only its
**Position** and **Status** filled in; 61 bytes, three frames, 16.1 s:

```
{0: 5, 1: h'8aec0d0e', 2: "KK4ODA", 4: 1794667200, 7: h'4e590e55b64e4d89',
 8: {1: h'5cf82a001bbfabf1', 2: {1: [337812300, -843876500], 5: 0}}}
```

A report of 512 bytes, the most, takes sixteen frames, 85.8 s: keep reports short.

Test vectors: `vectors/alerts.json`, `frames`.

---

## 4. Hearing one

A station that hears a broadcast (RP-4 §5) checks it in this order. The first check that fails
decides, and every refusal is silent: the program sounds nothing, shows nothing and answers
nothing.

1. **One of RP-8's.** The data decodes as deterministic CBOR to a map whose key 0 is 1, 2, 3, 4 or
   5. Otherwise it is *not an alert*: another program's broadcast, or a kind this program does not
   know (RP-0 §9.5).
2. **Its shape.** The keys keep §3's types and rules, and the frame is at most 512 bytes. Otherwise
   it is *malformed*.
3. **Its circle.** The station knows a circle with this ID. Otherwise: *unknown circle*.
4. **Its tag.** The tag, worked out again with that circle's key, is the frame's. Otherwise: *the
   tag does not check*. Compare in constant time.
5. **Its time.** `time` is within 600 seconds of the station's clock, either way. Otherwise: *out
   of time*. Stations' clocks must be right to a few minutes for alerts to work, which a radio
   clock or the Internet gives.
6. **For whom.** An alert or a report without `to` is for every station that knows its circle.
   Otherwise `to` has the same base (RP-0 §6.1) as one of the station's callsigns: an alert to
   `N0CALL` reaches `N0CALL-7`. Otherwise it is *for another station*: the station keeps nothing
   of it.
7. **New.** The station has not heard this tag in the last 1200 seconds. Otherwise it has *heard it
   again* (§5).

What passes is *an alert for this station*, *an alert for the circle*, a *heard*, *seen* or
*reply* for this station, *a report for this station* or *a report for the circle*. A program keeps each tag it hears for 1200 seconds, twice the window of
step 5, so a frame heard again is always known for one.

Why a short tag: 8 bytes leave a forger one chance in 2⁶⁴ a try, and every try is a transmission
the circle can hear. The tag is a check of the circle, not of the person: any member can make any
member's alert (§7).

Test vectors: `vectors/alerts.json`, `hearing`.

---

## 5. Answering

* **Heard.** A station that hears an alert for it sends `heard` at once, by itself, whether or not
  its operator is there. It sends it again when it hears the same alert again (step 7), unless its
  `heard` to that alert has not gone out yet: a sender still sending is one whose first `heard` was
  lost, or that has not heard it yet. To an alert for the circle, every station that hears it waits
  a random time from 0 to 10 seconds first, so their answers are less likely to collide.
* **Seen.** When the operator acknowledges the alert, the station sends `seen`.
* **Reply.** The operator may answer with a line of text: a `reply`.

An answer goes in the alert's circle, tagged with its key: `to` is the alert's `from`, `re` the
alert's tag, `from` the answering station's callsign, `time` its own clock. A station never answers
a `heard`, a `seen` or a `reply`, and never an alert that was not for it.

**A report is never answered**, and its sender never sends it again for want of an answer: the
next report carries the news. A report gets no copies to outlast a scan either; a station that
scans may miss one, and hear the next.

**The dial.** An answer goes on the dial the alert was heard on, which the modem says (RP-4 §6's
`dial_hz`): the `heard` at once, and `seen` or a reply whenever the operator acts, though a
station that scans its channels has moved on by then. The answer's `broadcast` names that dial, and
a modem that scans takes its scan there to send it. A modem that says no dial sends it where the
radio is.

**The sender.** A station that sent an alert to one station and has not heard its `heard` within 60
seconds of the broadcast going out (`broadcast-sent`, RP-4 §6) sends the same frame once more. An
alert for the circle is sent again once if no station's `heard` came. After that the program tells
its operator the alert was not heard.

**Copies, to outlast a scan.** A station that scans its channels stays a few seconds on each, and
an alert takes longer than that on the air: a scan that arrives partway through one cannot join it,
since the station needs every piece. So a sender that knows the station it alerts scans the dial it
sends on sends copies of the frame, one after another, until `scan_s` seconds (RP-5 §4.4) have
passed since the first went out, and stops as soon as a `heard` comes. It knows from the gateway
lists it reads: a listed gateway whose callsign has the base of `to` (RP-0 §6.1) and whose
channel on that dial has a `scan_s`. An alert for the circle goes on for the longest `scan_s` of
any listed gateway that scans the dial, since a member may run one. The copies are one frame: a
station takes the first it hears whole, and the rest are heard again (§4 step 7). The sending
again after 60 seconds is a sending of copies too.

---

## 6. Carrying alerts and reports (RP-4)

A frame is a broadcast's `data`. A host sends it with `broadcast` once it has callsigns (RP-4 §4);
the frame's `from` is the callsign it gave first. Every host that has said `hello` hears every
broadcast, callsigns or none (RP-4 §6), so a program can listen for alerts on a connection that
holds no callsign and answers no call.

Only modems that carry broadcasts (`caps.broadcast`) carry alerts and reports: of those RP-4 §9
maps, Aether. A program on another modem cannot alert or be alerted, or report, and says so.

With `caps.dial` a broadcast heard carries the dial it was heard on, and a `broadcast` may name the
dial it belongs on (RP-4 1.5.0 §6): how an answer goes back to the alert's dial (§5).

---

## 7. What a program does with an alert *(informative)*

An alert is meant to be hard to miss. The RadioPost client:

* sounds an alarm again every 20 seconds until its operator acknowledges the alert (which sends
  `seen`), brings its window forward, and offers quick replies (*Received*, *Calling you now*,
  *Can't now*) and a line of the operator's own;
* lets the operator choose, for each circle, between sounding the alarm and showing the alert
  quietly, and sounds through quiet hours only for alerts;
* sounds an alarm for at most five alerts from one callsign in ten minutes, and shows the rest
  quietly, since a member's station gone wrong should not keep the others' alarms ringing;
* shows the sender each alert's answers: sent, heard, seen, replied, or not heard;
* sends an alert on a **dial**: when `to` names a station with a listed gateway, the gateway's
  channel the client predicts best, else the circle's **watch dial**, else wherever the radio is.
  It tunes the radio there to send, and leaves it there to hear the answers;
* lets each circle have a watch dial, agreed with the others like the keyword. A member's station
  that is not a gateway keeps its radio there; one whose gateway scans has the watch dial among
  the gateway's channels, which the client checks, since an automatic answer must go in an
  automatic-control segment (§8);
* puts the dial an alert was heard on beside it, and **Call back** fills the call's dial with it;
* keeps each circle's keyword, hidden until its operator clicks **Show**, so the operator can tell
  it in person to someone joining; a circle kept before it kept keywords gets its keyword when the
  keyword is typed again;
* deletes a circle when asked, its key and keyword with it: its alerts and reports still waiting
  are not sent, answers to its alerts are not sent, and a report sent again stops.

Any member of a circle can make an alert from any callsign: the tag says the alert came from the
circle, not from whom. A circle of two members knows. Where proof of the person matters, that is
mail, which RP-1 signs.

A gateway program does not alert and does not answer alerts: nobody is there to hear the alarm.

**Reports.** The RadioPost client:

* sends a form as a report from its composer, **Broadcast to** a circle, to the whole circle or
  to one station, on the same dial an alert to it would take;
* sends it once, or again every 10, 15, 30 or 60 minutes. Each time it puts the station's position
  as it is then into the fields that held the station's own (W55), with a new `time`, so a net
  sees a moving station move. Repeats go only while the program's window is open, stop after four
  hours unless the operator starts them again, and stop at once on **Stop**;
* shows each report heard on its map of reports, by the circle's name and the sender's callsign,
  as a form in a message is shown, and keeps the last 200.

A gateway program neither sends nor shows reports.

---

## 8. The rules on the air *(informative)*

* **Nothing is encrypted** (§97.113(a)(4); RP-0 §7). The frame is published CBOR; the text is
  plain. The tag hides nothing: it is a check anyone can see and nobody outside the circle can
  make. The keyword never goes on the air.
* **Answers by themselves are automatic transmissions.** A `heard` goes whether or not the
  operator is at the station. Where the rules limit automatic control to some segments (in the US,
  §97.221), a station whose operator is away should not answer outside them: a program offers to
  turn `heard` off, and a modem that checks the band rules (`caps.regulatory`, RP-4 §8) refuses
  what they do not allow.
* **Identification** is the modem's (RP-4 §8): Aether's datagrams carry the station's callsign in
  the signal itself.
* **Reports sent again are the operator's.** A report repeated every few minutes is sent under the
  operator's control, not automatic control: a program repeats one only while its operator is at
  it (the window open, as in §7), and for a few hours at most. A station under automatic control
  sends none.

---

## 9. Test vectors

`vectors/alerts.json`:

* `circles`: two circles' keywords, normalised, with their keys, IDs and checks (§2);
* `keywords`: five keywords as typed, normalised, kept or refused;
* `frames`: Facundo's alert to Ana, her station's `heard`, her `seen` and her reply, his alert to
  the whole circle, his Position Report to Shelter 3's circle and his Storm Report to its net
  control, byte for byte (§3, §5);
* `hearing`: broadcasts heard by stations with given callsigns, circles and clock, each with its
  verdict from §4, the reports' among them.

---

## Change log

| Version | Date | Change |
|---|---|---|
| 0.1.0 | 2026-11-18 | First draft (wishlist W52): circles from a keyword, the alert frame and its tag, the order of checks, the answers, the sender trying again, carried as RP-4 broadcasts. Sizes and air times measured. |
| 0.2.0 | 2026-11-21 | An alert to a station whose gateway scans (D46): §5, answers go on the dial the alert was heard on; copies sent for the gateway's `scan_s`, stopped by a `heard`; a `heard` sent again unless the last has not gone out. §6, the dial (RP-4 1.5.0). §7, the dial an alert goes on and a circle's watch dial. No frame or check changes, and the test vectors stand. |
| 0.3.0 | 2026-11-24 | Reports ([the forms design](../docs/design/06-forms.md), step 5; wishlist W33): §3, kind 5 `report` and key 8 `form`, RP-2 §8.2's map; a frame of any kind at most 512 bytes; §4, a report's verdicts; §5, a report is never answered, nor sent again for want of an answer; §7, sending one once or again every few minutes, its positions brought up to date; §8, repeats under the operator's control. Title *Alerts and reports*. Test vectors: the reports' frames and their hearing. |
| 0.4.0 | 2026-11-28 | §2: a program may keep a circle's keyword, shown only when its operator asks and sent nowhere, so the operator can tell it to someone joining. §7: the client keeps it hidden behind **Show**, and deleting a circle stops what was still to go for it. No frame or check changes, and the test vectors stand. |
