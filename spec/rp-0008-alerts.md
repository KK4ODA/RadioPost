# RP-8: Alerts

| | |
|---|---|
| **Status** | Draft |
| **Version** | 0.1.0 |
| **Updated** | 2026-11-18 |
| **Editor** | KK4ODA |
| **Licence** | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| **Depends on** | RP-0, RP-4 |

How one station tells another, or a group of stations, that it needs them now: a line of text that
the other station's program shows as an alarm, with a way to answer. An alert is a **broadcast**
(RP-4 §6): it needs no call and no session, so it reaches a station over a path too weak to connect
on, and it can reach several stations at once.

Only stations that share a **keyword** can alert each other. The keyword makes a key, and every
alert carries a short **tag** that only that key can make. Nothing is encrypted: anyone listening
can read every alert, and only the stations that know the keyword can make one that their programs
will sound for (RP-0 §7).

---

## 1. Model

* A **circle** is the stations whose operators typed the same keyword into their programs: a
  family, a net, a team. The keyword gives the circle's key (§2). An operator may be in several
  circles; each is a keyword.
* An **alert** is from one station's operator, to one station or to the whole circle. It carries a
  line of text, the time, and a tag made with the circle's key (§3).
* The station alerted answers it (§5): **heard** at once, by itself; **seen** when its operator
  acknowledges it; and a **reply**, a line of text, if the operator writes one.
* A station that hears a broadcast checks it (§4). A frame that is not an alert, that names a circle
  the station does not know, or whose tag does not check, is dropped without a sound.

Alerts are for the moment: a few minutes. Anything that must arrive later, or must be kept, goes
as mail (RP-2).

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
key, not the keyword.

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

An alert, and each answer to it, is one CBOR map (RP-0 §4.1), sent as a broadcast's `data` (RP-4
§6). Its key 0 names the kind of broadcast, from RP-0 §9.5; RP-8 defines four:

| `t` | Kind | What it is |
|---|---|---|
| 1 | alert | An alert (§1). |
| 2 | heard | The station alerted has heard the alert (§5). |
| 3 | seen | Its operator has seen it (§5). |
| 4 | reply | Its operator's line of text in answer (§5). |

| Key | Name | Type | Kinds | Meaning |
|---|---|---|---|---|
| 0 | `t` | uint | all | The kind, above. |
| 1 | `circle` | bstr (4) | all | The circle's ID (§2). |
| 2 | `from` | tstr | all | The sender's callsign (RP-0 §6.1), in upper case. |
| 3 | `to` | tstr | all; an alert may leave it out | The station the frame is for. An alert without it is for every station in the circle. An answer is for the alert's `from`. |
| 4 | `time` | uint | all | When it was sent, Unix seconds (RP-0 §4.4). |
| 5 | `text` | tstr | alert and reply only | One line: 1 to 100 bytes of UTF-8, in NFC, with no control characters (U+0000 to U+001F, U+007F to U+009F). |
| 6 | `re` | bstr (8) | heard, seen and reply only | The tag of the alert it answers. |
| 7 | `tag` | bstr (8) | all | The tag, below. |

`text` is present in an alert and a reply and absent from the others, and `re` is present in the
three answers and absent from an alert. A receiver ignores keys it does not know (RP-0 §4.1); the
tag covers them too. Keys 0–63 are the specifications' (RP-0 §4.1).

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

Test vectors: `vectors/alerts.json`, `frames`.

---

## 4. Hearing one

A station that hears a broadcast (RP-4 §5) checks it in this order. The first check that fails
decides, and every refusal is silent: the program sounds nothing, shows nothing and answers
nothing.

1. **An alert at all.** The data decodes as deterministic CBOR to a map whose key 0 is 1, 2, 3 or
   4. Otherwise it is *not an alert*: another program's broadcast, or a kind this program does not
   know (RP-0 §9.5).
2. **Its shape.** The keys keep §3's types and rules. Otherwise it is *malformed*.
3. **Its circle.** The station knows a circle with this ID. Otherwise: *unknown circle*.
4. **Its tag.** The tag, worked out again with that circle's key, is the frame's. Otherwise: *the
   tag does not check*. Compare in constant time.
5. **Its time.** `time` is within 600 seconds of the station's clock, either way. Otherwise: *out
   of time*. Stations' clocks must be right to a few minutes for alerts to work, which a radio
   clock or the Internet gives.
6. **For whom.** An alert without `to` is for every station that knows its circle. Otherwise `to`
   has the same base (RP-0 §6.1) as one of the station's callsigns: an alert to `N0CALL` reaches
   `N0CALL-7`. Otherwise it is *for another station*: the station keeps nothing of it.
7. **New.** The station has not heard this tag in the last 1200 seconds. Otherwise it has *heard it
   again* (§5).

What passes is *an alert for this station*, *an alert for the circle*, or a *heard*, *seen* or
*reply* for this station. A program keeps each tag it hears for 1200 seconds, twice the window of
step 5, so a frame heard again is always known for one.

Why a short tag: 8 bytes leave a forger one chance in 2⁶⁴ a try, and every try is a transmission
the circle can hear. The tag is a check of the circle, not of the person: any member can make any
member's alert (§7).

Test vectors: `vectors/alerts.json`, `hearing`.

---

## 5. Answering

* **Heard.** A station that hears an alert for it sends `heard` at once, by itself, whether or not
  its operator is there. It sends it again each time it hears the same alert again (step 7), since
  the sender is trying again because its first `heard` was lost. To an alert for the circle, every
  station that hears it waits a random time from 0 to 10 seconds first, so their answers are less
  likely to collide.
* **Seen.** When the operator acknowledges the alert, the station sends `seen`.
* **Reply.** The operator may answer with a line of text: a `reply`.

An answer goes in the alert's circle, tagged with its key: `to` is the alert's `from`, `re` the
alert's tag, `from` the answering station's callsign, `time` its own clock. A station never answers
a `heard`, a `seen` or a `reply`, and never an alert that was not for it.

**The sender.** A station that sent an alert to one station and has not heard its `heard` within 60
seconds of the broadcast going out (`broadcast-sent`, RP-4 §6) sends the same frame once more. An
alert for the circle is sent again once if no station's `heard` came. After that the program tells
its operator the alert was not heard.

---

## 6. Carrying alerts (RP-4)

A frame is a broadcast's `data`. A host sends it with `broadcast` once it has callsigns (RP-4 §4);
the frame's `from` is the callsign it gave first. Every host that has said `hello` hears every
broadcast, callsigns or none (RP-4 §6), so a program can listen for alerts on a connection that
holds no callsign and answers no call.

Only modems that carry broadcasts (`caps.broadcast`) carry alerts: of those RP-4 §9 maps, Aether.
A program on another modem cannot alert or be alerted, and says so.

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
* shows the sender each alert's answers: sent, heard, seen, replied, or not heard.

Any member of a circle can make an alert from any callsign: the tag says the alert came from the
circle, not from whom. A circle of two members knows. Where proof of the person matters, that is
mail, which RP-1 signs.

A gateway program does not alert and does not answer alerts: nobody is there to hear the alarm.

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

---

## 9. Test vectors

`vectors/alerts.json`:

* `circles`: two circles' keywords, normalised, with their keys, IDs and checks (§2);
* `keywords`: five keywords as typed, normalised, kept or refused;
* `frames`: Facundo's alert to Ana, her station's `heard`, her `seen` and her reply, and his alert to
  the whole circle, byte for byte (§3, §5);
* `hearing`: thirty broadcasts heard by stations with given callsigns, circles and clock, each with
  its verdict from §4.

---

## Change log

| Version | Date | Change |
|---|---|---|
| 0.1.0 | 2026-11-18 | First draft (wishlist W52): circles from a keyword, the alert frame and its tag, the order of checks, the answers, the sender trying again, carried as RP-4 broadcasts. Sizes and air times measured. |
