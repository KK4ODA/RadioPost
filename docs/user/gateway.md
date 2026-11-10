# Running a gateway with RadioPost Gateway

How a licensed operator sets up a RadioPost gateway on the computer at their radio, registers it
with their post office, and leaves it answering. Users call the gateway by radio; it carries each
session to the user's own post office over the Internet (RP-3 §9, RP-5 §5).

---

## 1. What the gateway's computer needs

* **An HF radio and Aether HF 0.2.0-beta.146 or later**, set up and working with the radio as
  Aether's own manual says. Aether keys the radio, checks the band rules and sends the Morse
  identifier at the end of each session. Set Aether's callsign to the gateway's, with its SSID
  (say `KK4ODA-1`).
* **An Internet connection.** The gateway only makes outgoing connections, so a home router needs
  no change.
* **RadioPost Gateway**, from the [releases](https://github.com/KK4ODA/RadioPost/releases): the
  installer named `RadioPost Gateway` for Windows (`…-setup.exe` or `.msi`), macOS (`.dmg`) or
  Linux (`.deb` or `.AppImage`). The installers are not signed yet: on Windows, SmartScreen asks
  once (More info, then Run anyway); on macOS, open it with Control-click, then Open.
* **RadioPost**, the client, on the same computer, enrolled as you with your post office (its
  Setup, with the code your post office's operator gave you). The gateway registers in your name
  through it. It needs nothing else: you can keep using RadioPost on another computer.

## 2. Setup

Open RadioPost Gateway. Until it is set up and registered, it says what is missing; click **Set up
the gateway**.

**1. The gateway.** Enter its **Callsign**: yours with an SSID, such as `KK4ODA-1`. Leave
**Modem** at `ws://127.0.0.1:8515/v1`, where Aether listens on the same computer. **Country** is
where the gateway operates, two letters: it decides which third-party mail the gateway may hand
over (RP-6 §5.1). Leave *The modem does not check the band rules* clear: Aether checks them. Keep
*Answer calls when the app opens* ticked for a gateway left on its own. Click **Save**. The gateway
makes its key; it is shown under the form.

**2. Registration.** The form shows the account the RadioPost app on this computer is enrolled
as. Under **Channels**, give each dial frequency (MHz, upper sideband) where callers will find the
gateway, and Aether's bandwidth there (500 or 2300 Hz). **Grid** and **Place** help callers choose
a gateway near them; **Hours** says when it answers (`always`, or in words). Click **Register**.
Your post office lists the gateway for its users and issues it a credential, valid for 90 days.
Register again before it runs out (the same way: it replaces the registration); the Registration
box turns red two weeks ahead.

**3. Post offices (optional).** The gateway finds each user's post office in DNS. Name one here
only while its DNS records are not published yet: its domain and its API address
(`https://…/rp/v1`).

A post office's operator can also issue the gateway's credential by other means: paste it under
*A credential issued by other means* in step 2.

## 3. Running it

Click **Start**. The badge at the top says what the gateway is doing:

| Badge | Means |
|---|---|
| Stopped | Not answering. |
| Waiting for the modem | Aether is not running, or not reachable at the Modem address. The gateway tries again every five seconds; the Modem box says why. |
| Listening | It has claimed its callsign at Aether and answers calls. |
| Session | A user's session is under way. |

The **Post offices** box shows whether each post office the gateway knows of let it log in, at
the last check (every ten minutes while it answers, or **Check now**). **Registration** shows the
gateway's credential and when it runs out, in red from two weeks before. **Sessions** lists each session carried: who called,
the account, how long, the messages the caller sent and picked up, and how it ended. The log at
the bottom says what the gateway does.

**Stop** ends a session under way and lets the modem go. Settings change only while the gateway is
stopped. Closing the window stops the gateway too; while it answers, it asks first.

**Unattended.** With *Answer calls when the app opens* ticked, the gateway answers as soon as the
app opens. To have it open when you sign in: on Windows, put a shortcut to RadioPost Gateway in the
Startup folder (Win+R, `shell:startup`); on macOS, add it to Login Items (System Settings,
General); on Linux, to your desktop's startup applications.

## 4. The rules

* **Control.** A gateway left answering with nobody at the radio is under **automatic control**.
  Set Aether's control to Automatic: Aether then transmits only in the segments where automatic
  control is allowed (§97.221(b)) and refuses anywhere else, so register channels in those
  segments. While you are at the radio, local control is yours to choose.
* **Identification.** Aether sends the gateway's Morse identifier at the end of each session.
* **No encryption.** RadioPost signs; it never encrypts (RP-0 §5).
* **Third-party traffic.** The post office marks mail that is third-party traffic, and the gateway
  hands it only to stations in countries that allow it, by the gateway's **Country** (RP-6 §5.1).

## 5. A gateway with no screen

On a computer with no screen, run the gateway from the command line with the same directory and
panel:

```
radiopost-gateway panel
```

from the tools archive of the same release. It serves the panel at `http://127.0.0.1:8621/` for a
browser on that computer (or through an SSH tunnel: `ssh -L 8621:127.0.0.1:8621 the-gateway`), and
answers calls when the gateway is set to answer at start. What it does goes to its output too, so a
service manager keeps it in its log. A systemd unit, for example:

```ini
[Unit]
Description=RadioPost gateway
After=network-online.target

[Service]
ExecStart=/usr/local/bin/radiopost-gateway --dir /var/lib/radiopost-gateway panel
User=radio
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

`radiopost-gateway serve` answers calls with no panel at all, as before.

## 6. What is kept

The gateway's directory is `.radiopost-gateway` in your home directory (`$RADIOPOST_GATEWAY` names
another; Setup shows where):

| File | Holds |
|---|---|
| `gateway.json` | The settings. |
| `key.hex` | The gateway's key. Back it up: a new key needs a new registration. Never share it. |
| `credential.hex` | The gateway's credential, from its registration. |
| `held.json` | Callers' credentials seen, and pieces of uploads a broken link left (RP-3 §7). |
| `sessions.json` | The last 200 sessions carried, as the Sessions list shows them. |

## 7. When something is wrong

| You see | Do |
|---|---|
| Waiting for the modem: *cannot reach the modem* | Start Aether. Check the Modem address in Setup. |
| Waiting for the modem: *the modem refused* | Another program holds the callsign at Aether, or Aether refused it: Aether's log says why. |
| A post office *not found* | Its DNS records are not published: name it in Setup, step 3. |
| A post office *refused the gateway* | The credential has run out or was revoked: register again (Setup, step 2). |
| A session ended *nothing heard for 300 seconds* | The path faded or the caller left: the caller's mail is resumed on its next call (RP-3 §7). |

To report a problem, open an issue on [RadioPost's repository](https://github.com/KK4ODA/RadioPost/issues)
with the log lines and, for a session that failed, Aether's recording of it.
