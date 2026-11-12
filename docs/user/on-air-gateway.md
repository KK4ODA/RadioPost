# On-air tests: email through a gateway

How a tester sends and receives ordinary email by radio through the pilot gateway, KK4ODA-1, and
the project's post office, `radiopost.org` (Phase 2). Two stations take part: the tester's, with
the RadioPost app, and the gateway, which its operator runs with RadioPost Gateway
([the gateway's guide](gateway.md)). Station-to-station tests, with no gateway or post office, are
[a guide of their own](on-air-tests.md).

---

## 1. What a tester needs

* An HF radio and a computer with **Aether HF 0.2.0-beta.146 or later**, set up and working with
  the radio as Aether's own manual says. RadioPost uses Aether as its modem: Aether keys the radio,
  checks the band rules, and sends the Morse identifier at the end of each session. Aether's own
  callsign (its Setup step 1) is yours. RadioPost calls as the callsign it was enrolled with,
  whatever Aether's Setup says: there is nothing to copy into Aether. While RadioPost holds that
  callsign, from Aether 0.2.0-beta.149, Aether's Setup shows it greyed (*KK4ODA · RadioPost*).
* **Aether keying the radio itself** (Aether's Setup step 2: CAT, serial, rigctld, FLRig, CM108
  or VOX, not *Host*), with its ports as they are. RadioPost talks to Aether's control port,
  8515 (`ws://127.0.0.1:8515/v1`). The host programs port, 8300, is for Winlink Express and VarAC;
  RadioPost never uses it, and it stays at 8300. Close Winlink Express or VarAC while you test if
  they use the same callsign: one program at a time answers for a callsign at Aether.
* **RadioPost**, from the [releases](https://github.com/KK4ODA/RadioPost/releases): the installer
  named `RadioPost` for Windows (`…-setup.exe`), macOS (`.dmg`) or Linux (`.deb` or
  `.AppImage`). The installers are not signed yet: on Windows, SmartScreen asks once (More info,
  then Run anyway); on macOS, open it with Control-click, then Open.
  From 0.1.0-beta.5 on, RadioPost says when a newer release is out and updates itself:
  **Update and restart** at the top of its window.
* **The Internet, once**, to enroll and to fetch the list of gateways. The tests themselves go by
  radio.

## 2. An account at radiopost.org

1. Email `postmaster@radiopost.org` **from the address your licence record holds**, with your
   callsign and the name you want before the `@` (say `facundo` for `facundo@radiopost.org`).
   The post office's operator checks your licence and answers with a one-time **code**, good for
   seven days.
2. Open RadioPost. It opens on **Setup**. Under *Enroll with your post office*, enter the
   **Address** (`facundo@radiopost.org`), your **Callsign** and the **Code**, leave *Post office*
   empty, and click **Enroll**. RadioPost makes
   this computer's key, finds the post office in DNS, trusts its key from there, and keeps the
   credential it issues. The top of the window now shows your address and callsign, and Setup's
   first box says *Enrolled as …* with the date the credential runs out.

The credential lasts 90 days. In its last month RadioPost renews it by itself: over the Internet
while its window is open, or on a call to a gateway. The log at the bottom says when it did, and
says why once if it could not. **Renew credential** in Setup does it at once.

## 3. Find the gateway

1. In the *Station* area, choose **A gateway**, then **Choose a gateway**.
2. Click **Refresh over the Internet**. The list comes from your post office, its signature
   checked with the post office's key, and is kept for when there is no Internet.
3. Enter **Your grid** (say `EM73`) and **Save**: the nearest gateways come first.
4. Find **KK4ODA-1**: its channels (dial frequency, upper sideband, and Aether's bandwidth there)
   and its hours. Click **Choose**. Tune Aether to one of its channels, at that bandwidth.

## 4. Send an email by radio

1. Click **Write**. Address the message to an ordinary email address you can read (your own
   mailbox elsewhere is best), give it a subject and a few lines, and click **Put in outbox**.
   Writing to an address puts it on your accept list, so its answer will be let in. To and Cc
   take several addresses, separated by commas, as `maria@example.com` or
   `Maria <maria@example.com>`; RadioPost checks each, and says which one is wrong.
2. Set **Fetch** to **Everything waiting**, and click **Call**.

The log at the bottom says what happens: the call, each message delivered, what waits for you, and
the end. A session of a few short messages takes a minute or two at 2300 Hz on a fair path, longer
on a weak one. Within a minute of the session, the email arrives at the address you wrote to, from
your radiopost.org address, with a `Radio-Path:` header naming the gateway and the session.

## 5. Receive by radio

1. From that mailbox, answer the message. The answer waits at your post office.
2. **Call** KK4ODA-1 again. The answer comes down, and is in the **Inbox**.

To choose what comes, set **Fetch** to **Only the list of what waits** and call: nothing comes
but the list, which the **Waiting** folder shows. Tick what you want, and call again: what you
ticked comes, whatever Fetch says. Messages of 1 000 bytes or less on the air come anyway.

**Attachments.** An email whose attachments are too large for the radio comes without them: the
message says *Held at the post office* under each one. Click **Ask on the next call** beside one,
then call: the gateway answers *Your attachments are in a new message*, and the call after brings
them, from `postmaster@radiopost.org`. Each file must fit on the air on its own (64 kB): a larger
one is refused.

## 6. The rules

* **Control.** Your station is attended: you are at the controls (local control) for the whole
  test. The gateway may be unattended; its operator has set Aether for automatic control.
* **Identification.** Aether sends each station's Morse identifier at the end of the session, and
  nothing in Morse during it; §97.119(a) asks for the callsign at least every ten minutes, so keep
  each session under ten minutes.
* **No encryption.** RadioPost signs; it never encrypts. Anyone can read every message.
* **Third-party traffic.** Email between you and someone who is not a licensed amateur is
  third-party traffic: in the United States it may go between US stations. The post office marks
  such mail, and the gateway hands it only where its country allows. Do not send business
  messages.

## 7. What to report

For each test, open an issue on [RadioPost's repository](https://github.com/KK4ODA/RadioPost/issues)
with:

* the date and time (UTC), the band, frequency and bandwidth, and your grid square;
* what happened: which messages went and came, how long each session took, and anything that went
  wrong;
* your RadioPost log: the file `radiopost.log` in RadioPost's folder (`.radiopost` in your home
  folder: on Windows, `C:\Users\<you>\.radiopost`). It keeps what the bottom of the window says,
  with the time, after the window is closed: RadioPost 0.1.0-beta.8 and later, and from
  0.1.0-beta.9 without the addresses or subjects of your mail;
* for a session that failed or was slow, Aether's recording of it: switch on Aether's **Record**
  before the test, and send the files from Aether's Log tab (*Send files…*). From Aether
  0.2.0-beta.150 and RadioPost 0.1.0-beta.9, *Send files…* carries the RadioPost log too.

The gateway's operator sees each session too, in RadioPost Gateway's Sessions list.

## 8. Not covered yet

* **The Internet down.** Copies of your mail placed ahead at your usual gateways, and a gateway
  taking your mail while it cannot reach the post office, are Phase 3.
* **Forwarding by radio** between gateways is Phase 4.
