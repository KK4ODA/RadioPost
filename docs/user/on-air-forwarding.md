# On-air tests: forwarding between two gateways

How two gateway operators try forwarding by radio (RP-7) on the air: one gateway, cut off from
the Internet on purpose, hands its queue to a partner gateway that reaches the Internet, and
collects the mail the post office left there for its users. Both gateways stay on one shared
channel, so no radio is retuned (Phase 4's first trial). **This trial is deferred until
later**: the guide is ready for when it runs. Running a gateway is
[a guide of its own](gateway.md); so are [email through a gateway](on-air-gateway.md) and
[station-to-station tests](on-air-tests.md).

---

## 1. Who takes part

* **Gateway A**, the one that will be cut off. Its operator unplugs its Internet during the test.
* **Gateway B**, its partner, which keeps its Internet. It must be in reliable radio range of A on
  the shared channel: for a few hundred kilometres, 80 m by day or 40 m in the evening.
* **A tester**, with the RadioPost app, whose usual gateway is A, in radio range of A.
* **The post office's operator** (for `radiopost.org`, the project's), who sets one route.
* Someone outside, with an ordinary email address, to send the tester an email during the test.

## 2. What each gateway needs

* **RadioPost Gateway 0.1.0-beta.21 or later**, registered with `radiopost.org` and answering
  ([the gateway's guide](gateway.md), §2–§3).
* **Copies placed ahead** on (Setup, step 1), so that the post office copies its users' mail
  there.
* **Aether HF** keying the radio itself, with the gateway as its only program. Both radios on the
  **shared channel**, inside a segment where an unattended station may start a data transmission
  (US: §97.221(b); [the gateway's guide](gateway.md), §4, lists them). For example 7.1010 MHz USB
  with Aether at 2 300 Hz (it occupies 7.10124 to 7.10376 MHz), or 3.5900 MHz USB.
* Its **gateway log** kept (`gateway.log` in the gateway's directory, §7 of the gateway's guide),
  and Aether's **Record** on for the test.

## 3. Before the day

1. **Name each other.** On each gateway, stopped: **Partners**, **Show**, type the other's
   callsign, **Add**.
2. **B's window.** On B, stopped: in the Partners view, under **Windows**, add one on the shared
   channel: for example 7.1010 MHz, 2 300 Hz, every hour, at 00:15, for 10 minutes. Then
   **register again** in Setup, so the window is in B's record at the post office. A cut-off A
   calls B only inside it.
3. **The route.** The post office's operator sets it: `radiopost-po route --via B --users-at A`
   (with the two callsigns). From then on, when A is cut off, the mail for A's users waits at B.
4. **A learns B's window.** Restart A (Stop, Start) once B has registered again. A's Partners list
   then shows B's **next window**. *None published* means A has not fetched B's record yet: check
   B registered, and restart A.
5. **The tester** names A as a usual gateway in RadioPost's Setup, and calls A once with A online,
   to see that it works.

## 4. The test

Plan it around B's window: A must be cut off at least 20 minutes before it opens, since the post
office counts A cut off only after 15 minutes without hearing from it. With a window at :15, cut
A off before :55 the hour before.

1. **Cut A off.** Unplug the Internet from A's computer (its network cable, or its Wi-Fi), leaving
   Aether and the radio as they are. Within ten minutes A's **Post offices** box shows the check
   failing. Note the time.
2. **The tester sends.** The tester calls A by radio and sends an email to someone outside.
   RadioPost shows it *queued at* A. A's **Held** list shows one session waiting.
3. **Mail comes in.** Someone outside emails the tester. Fifteen minutes after A went quiet, the
   post office leaves that email at B for A. B's Partners list shows it *held for* A.
4. **A calls B in B's window.** When the window opens, A calls B by itself. Its log says *calling
   B in its window to forward the queue*. In one session A hands over the tester's session with
   its email, and collects the email held for the tester. B reports the session: the email goes
   out to its recipient. A's Partners list shows how the call went.
   * If B does not answer, A tries twice more, a minute apart, then waits for the next window.
   * A never calls while Aether reports the channel busy.
   * Rather than wait, A's operator may press **Forward now** beside B. An operator at the radio
     may call at any time.
5. **The tester picks up.** The tester calls A again, A still cut off. A offers the email it
   collected from B, as any other.
6. **A comes back.** Plug A's Internet back in. Its next poll tells the post office: what was left
   at B for A is dropped. The tester's queued email shows *sent* once the tester next calls a
   gateway that reaches the Internet.

## 5. The rules

* **Control.** Each gateway left answering with nobody at the radio is under automatic control:
  set Aether's control to Automatic, which keeps it to the segments where that is allowed. The
  shared channel and B's window must lie inside one (§97.221(b)). While an operator is at a
  gateway's radio, local control is theirs to choose.
* **Identification.** Aether sends each station's Morse identifier at the end of each session.
* **No encryption.** RadioPost signs; it never encrypts (RP-0 §5). Anyone can read every message.
* **Third-party traffic.** The tester's emails are third-party traffic. Between two US gateways
  and a US tester they are allowed (§97.115); keep this test between stations in the United States.

## 6. What to report

Open an issue on [RadioPost's repository](https://github.com/KK4ODA/RadioPost/issues) with:

* the date, the channel and Aether's bandwidth, both gateways' callsigns and grid squares, and the
  distance between them;
* the times (UTC): A cut off, the tester's email queued, the email in, A's call or calls to B and
  how long each took, A back;
* what arrived where: the outside recipient's email, the tester's;
* both gateways' `gateway.log`, and Aether's recordings of any call that failed or was slow (*Send
  files…* in Aether's Log tab carries both).

## 7. Not covered yet

* **Tuning.** This trial keeps both gateways on one channel. A gateway whose modem can set the
  dial (RP-4 §4.1) tunes to its partner's window and back; that is for a later trial.
* **More than two gateways.** A record may pass through up to eight gateways, each cut off
  handing it on to its own partners (RP-7 §3.4); this test uses one hop.
* **Choosing between partners.** With two partners, a cut-off gateway keeps its queue for the one
  that reached the Internet at its last call, when it opens a window within the hour (RP-7 §6.2).
  The conformance tests check it (F-04); on the air it needs three gateways.
