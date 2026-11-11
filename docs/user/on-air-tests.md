# On-air tests: two stations, station to station

How two operators carry RadioPost mail between their stations over the air with Aether HF, before
any post office or gateway exists (Phase 1). It takes two stations, an hour, and one person who
acts as the test post office.

---

## 1. What each station needs

* An HF radio and a computer with **Aether HF 0.2.0-beta.146 or later**, set up and working with
  the radio as Aether's own manual says. RadioPost uses Aether as its modem: Aether keys the radio,
  checks the band rules, and sends the Morse identifier at the end of each session.
* **RadioPost**, from the [releases](https://github.com/KK4ODA/RadioPost/releases): the installer
  for Windows (`…-setup.exe`), macOS (`.dmg`) or Linux (`.deb` or `.AppImage`). The
  installers are not signed yet: on Windows, SmartScreen asks once (More info, then Run anyway); on
  macOS, open RadioPost with Control-click, then Open.
  From 0.1.0-beta.5 on, RadioPost says when a newer release is out and updates itself:
  **Update and restart** at the top of its window.

RadioPost finds Aether at `ws://127.0.0.1:8515/v1`, where Aether listens unless set otherwise. The
**Modem** box in RadioPost's Station area says where; change it only if Aether's control port is
not the usual one.

## 2. Credentials, before going on the air

A RadioPost station proves who it is with a **credential**: its device key, its operator's callsign
and its address, signed by a post office. Until the post office service exists (Phase 2), one
person issues credentials for the test with `radiopost-issue`, from the tools archive of the same
release. Never use the keys in `vectors/`: their private halves are published.

**Each tester**

1. Open RadioPost. It opens on **Setup**.
2. Under *This device's key*, click **Make a key**, and send the long hexadecimal key shown to the
   test post office's operator, with your callsign and the name you want before the `@`. The key
   is public: email is fine.

**The test post office's operator**, once:

```
radiopost-issue key --domain test.radiopost.org --out test-po.json
```

`test-po.json` is the post office's secret: keep it, and back it up. The command prints a line
like `radiopost trust test.radiopost.org 5f1d… 1794… 1825…`: send its domain and key to every
tester. Then, for each tester:

```
radiopost-issue issue --key test-po.json --address facundo@test.radiopost.org \
    --operator KK4ODA --device <the tester's key>
```

and send the tester the credential it prints (hexadecimal). It is valid for 90 days.

**Each tester**, back in **Setup**:

3. Paste the credential under *Credential* and click **Keep credential**. The top of the window
   now shows your address and callsign.
4. Under *Post offices trusted*, enter the domain (`test.radiopost.org`) and the key the operator
   sent, and click **Trust**. A station trusts the post offices its operator chooses (RP-1 §7.3).

## 3. A rehearsal over the Internet (optional)

Before the air, the two stations can run the same session over the Internet, to check the
credentials and the trust with no radio involved. Each runs `radiopost-tcp`, from the tools
archive, and puts its address, `127.0.0.1:8600`, in RadioPost's **Modem** box.

**Station A** (it answers): `radiopost-tcp --listen 0.0.0.0:8774`. The first time, it makes a
certificate for itself and prints its fingerprint (`SHA-256 5c0e…`): send that to B. A's router
must pass TCP port 8774 to this computer. In RadioPost, click **Listen**.

**Station B** (it calls): `radiopost-tcp --trust <A's fingerprint> --peer <A's callsign>=<A's
address>:8774`, then chooses **Another client's station** and **Call**s A's callsign.

The link is TLS: encryption is allowed on the Internet, which is not the air. Put
`ws://127.0.0.1:8515/v1` back in the **Modem** box afterwards. (On an amateur mesh network such as
AREDN, which is on the air, both use `--plain`: nothing on the air is encrypted.)

## 4. On the air

Agree a frequency in the band's digital segment, a time, and who calls first. Start Aether on
both stations and check it works with the radio (its *Tune* or a probe), at the bandwidth you
agree (500 or 2300 Hz, Aether's setting). Set each station's Aether to the callsign of its
credential (say KK4ODA at one station and WD5EMA at the other): RadioPost answers and calls as
that callsign, and Aether hands it every call to it while RadioPost runs.

**Station A** (it answers): in RadioPost's *Station* area, click **Listen**. The badge at the top
says *Listening*.

**Station B** (it calls):

1. Click **Write**, address the message to A's address (`ana@test.radiopost.org`), give it a
   subject and a few lines, and click **Put in outbox**.
2. In *Station*, choose **Another client's station**, enter A's callsign under *Call*, and
   click **Call**.

The log at the bottom of each window says what happens: the call, each station proving who it is,
each message delivered or received, and the end. A session of a few short messages takes a minute
or two at 2300 Hz on a fair path, longer on a weak one. Aether's own window shows the link and
the identifier at the end.

Then look: A's **Inbox** has B's message, and B's **Sent** has it. Swap roles; send a message each
way in one session (each writes one to the other before the call); attach a small file (a few
hundred bytes: every byte goes on the air). After each session, click **Listen** again to answer
the next call.

Closing RadioPost while it listens or a session is under way asks first: closing ends the
session and leaves the other station waiting.

## 5. The rules

* **Control.** Each station is attended: its operator is at the controls (local control) for the
  whole test. Aether checks each transmission against the band rules it knows, and refuses one
  they do not allow.
* **Identification.** Aether sends each station's Morse identifier at the end of the session: always
  for a US callsign (§97.119), and as its settings say for others. RadioPost adds nothing on the air
  of its own. Aether sends nothing in Morse during a session, and §97.119(a) asks for the callsign
  at least every ten minutes, so keep each session under ten minutes: a few short messages, small
  files.
* **No encryption.** RadioPost signs; it never encrypts (RP-0 §5). Anyone can read every message.
* **Third-party traffic.** Messages between the two operators themselves are not third-party
  traffic. Do not carry mail for others in these tests.

## 6. What to report

For each test, open an issue on [RadioPost's repository](https://github.com/KK4ODA/RadioPost/issues)
with:

* the date and time (UTC), the band and frequency, Aether's bandwidth, and the two stations'
  locations (grid squares);
* the result: which messages arrived, how long the session took, and anything that went wrong;
* each station's RadioPost log: copy the lines at the bottom of the window;
* for a session that failed or was slow, Aether's recording of it: switch on Aether's **Record**
  before the test, and send the files from Aether's Log tab (*Send files…*).

## 7. What these tests do not cover yet

* **Post offices and gateways.** Mail reaches only the other station. Each station keeps a report
  of the session (the panel counts them), for its post office when there is one; that is when a
  copy to anyone else (a `Cc`, an email address) goes on. Email through a gateway has
  [a guide of its own](on-air-gateway.md).
* **Renewal.** A credential lasts 90 days; the operator issues a new one after that.
* **One session at a time.** **Listen** answers one call, then stops.
