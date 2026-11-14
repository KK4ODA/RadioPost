# Running a gateway with RadioPost Gateway

How a licensed operator sets up a RadioPost gateway on the computer at their radio, registers it
with their post office, and leaves it answering. Users call the gateway by radio; it carries each
session to the user's own post office over the Internet (RP-3 §9, RP-5 §5). When the Internet is
down, it goes on: it checks callers itself, hands them the copies of their mail it holds, keeps
what they send, and reports it all when the Internet is back (§4).

---

## 1. What the gateway's computer needs

* **An HF radio and Aether HF 0.2.0-beta.146 or later**, set up and working with the radio as
  Aether's own manual says. Aether keys the radio, checks the band rules and sends the Morse
  identifier at the end of each session.
* **One place for the gateway's callsign: RadioPost Gateway's Setup** (below). Aether's own
  callsign, in its Setup step 1, is yours, and need not match the gateway's; do not copy the
  gateway's into Aether.
  * While RadioPost Gateway runs, it answers for the station. Aether answers only the gateway's
    callsign, and from Aether 0.2.0-beta.149 its Setup shows it in place of its own, greyed:
    *KK4ODA-1 · RadioPost Gateway*.
  * When the gateway stops, Aether's own callsign comes back.
* **Aether keying the radio itself**: in Aether's Setup step 2, CAT, serial, rigctld, FLRig,
  CM108 or VOX, not *Host*. Keyed by a host program, Aether transmits only through that program,
  and RadioPost's sessions do not reach the air. On a computer that also runs RMS Trimode with
  Aether set that way, close Trimode and change Aether's keying; an Aether profile for each makes
  the switch one click.
* **Aether's ports left as they are.** Aether listens on two ports, and RadioPost uses one:

  | Port | What it is | Who uses it |
  |---|---|---|
  | 8515 | Aether's control port | RadioPost (client and gateway), Aether's own window |
  | 8300, and 8301 for data | Aether's host programs port, the VARA-style one | RMS Trimode, Winlink Express, VarAC |

  RadioPost Gateway's **Modem** address, `ws://127.0.0.1:8515/v1`, is the control port. Nothing
  about the host programs port is set in RadioPost. Keep it at 8300, and never at 8515: on the
  control port's number, Aether stops at start (*host interface: 127.0.0.1:8515 is already in
  use*).
* **An Internet connection.** The gateway only makes outgoing connections, so a home router needs
  no change.
* **RadioPost Gateway**, from the [releases](https://github.com/KK4ODA/RadioPost/releases): the
  installer named `RadioPost Gateway` for Windows (`…-setup.exe`), macOS (`.dmg`) or
  Linux (`.deb` or `.AppImage`). The installers are not signed yet: on Windows, SmartScreen asks
  once (More info, then Run anyway); on macOS, open it with Control-click, then Open.
* **RadioPost**, the client, on the same computer, enrolled as you with your post office (its
  Setup, with the code your post office's operator gave you). The gateway registers in your name
  through it. It needs nothing else: you can keep using RadioPost on another computer. Each
  computer enrolls with a code of its own, since each has its own key: an enrollment on another
  computer does not count here, and copying its `.radiopost` folder would put one key on two
  computers.

## 2. Setup

Open RadioPost Gateway. Until it is set up and registered, it says what is missing; click **Set up
the gateway**.

**1. The gateway.** Enter its **Callsign**: yours with an SSID, such as `KK4ODA-1`. Leave
**Modem** at `ws://127.0.0.1:8515/v1`, where Aether listens on the same computer. **Country** is
where the gateway operates, two letters: it decides which third-party mail the gateway may hand
over (RP-6 §5.1). Leave *The modem does not check the band rules* clear: Aether checks them. Keep
*Answer calls when the app opens* ticked for a gateway left on its own. Tick *Hold copies of
users' mail, for when the Internet is down* for a gateway with room to spare (§4), and say for how
many users, how many days, and the largest message it holds with its text. Click **Save**. The
gateway makes its key; it is shown under the form. Change the copies setting later, and register
again (step 2) so that the post office knows.

**2. Registration.** The form shows the account the RadioPost app on this computer is enrolled
as. **RadioPost app's home** is the folder where that app keeps its key and credential
(`.radiopost` in your user folder): leave it. Under **Channels**, give each dial frequency (MHz, upper sideband) where callers will find the
gateway, and Aether's bandwidth there (500 or 2300 Hz). **Grid** and **Place** help callers choose
a gateway near them; **Hours** says when it answers (`always`, or in words). Click **Register**.
Your post office lists the gateway for its users and issues it a credential, valid for 90 days.
While the gateway answers, it renews that credential by itself in its last month, with your
account's post office, and uses the new one from its next session; the log says so. If renewing
keeps failing (the log says why, once), register again before the credential runs out (the same
way: it replaces the registration); the Registration box turns red two weeks ahead.

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
| Waiting for the modem | Aether is not running, is restarting (saving Aether's Setup restarts it), or is not reachable at the Modem address. The gateway tries again every five seconds and claims its callsign again once Aether is back; the Modem box says why. |
| Listening | It has claimed its callsign at Aether and answers calls. |
| Session | A user's session is under way. |

The **Post offices** box shows whether each post office the gateway knows of let it log in, at
the last check (every ten minutes while it answers, or **Check now**). **Registration** shows the
gateway's credential and when it runs out, in red from two weeks before. **Sessions** lists each session carried: who called,
the account, how long, the messages the caller sent and picked up, and how it ended. The log at
the bottom says what the gateway does.

**Held** says how many copies placed ahead the gateway holds, for how many users, and how many
sessions carried while the Internet was down wait to be reported; **Show** lists them (§4).

**Stop** ends a session under way and lets the modem go. Settings change only while the gateway is
stopped. Closing the window stops the gateway too; while it answers, it asks first.

**Unattended.** With *Answer calls when the app opens* ticked, the gateway answers as soon as the
app opens. To have it open when you sign in: on Windows, put a shortcut to RadioPost Gateway in the
Startup folder (Win+R, `shell:startup`); on macOS, add it to Login Items (System Settings,
General); on Linux, to your desktop's startup applications.

**Updates.** When a newer release is out, a line at the top of the window says so (the gateway
asks when it opens and every six hours). **Update and restart** stops the gateway, downloads the
update, checks that the project's update key signed it, installs it and opens the app again,
answering as before. On a Linux `.deb` install it asks for your password, as installing a package
does.

## 4. When the Internet is down

**Copies placed ahead** (RP-5 §5.8). Users name their usual gateways in their RadioPost's Setup.
When this gateway holds copies (Setup, step 1, and registered), their post office copies the mail
waiting for them here as it arrives, and the gateway keeps it: a message up to the size you set
with its text, a larger one listed only. The gateway asks its own post office for copies without
stopping, and any other post office that says it places copies here; it only makes outgoing
connections, so a home router needs no change. A copy goes when its user picks the message up,
here or anywhere, or after the days you set.

**A session while cut off** (RP-3 §9, RP-7 §1). When a caller's post office cannot be reached,
the gateway answers from what it holds, and says so to the caller in a notice:

* it checks the caller's credential and signature itself, with the post office's keys and list of
  revoked credentials, which it fetches and keeps while online and refreshes every six hours
  (RP-1 §7.3). A caller from a post office it never fetched is refused (*unknown post office*). A
  post office named in Setup step 3 is anchored on the key that signed the gateway's own
  credential; others come from DNS, and one hop of their lists of post offices;
* it hands the caller the copies it holds for them; third-party mail only as the rules allow
  (§5);
* it keeps what the caller sends in its queue, written to disk before it tells the caller it has
  it. The caller's RadioPost shows those messages *queued* at this gateway, and offers them again
  until a gateway that reaches the post office confirms them;
* held attachments cannot be asked for: the notice says so.

**When the Internet is back** (RP-5 §5.6). The gateway reports each session it carried while cut
off to the caller's post office, oldest first: the messages go on, the pickups are marked, and the
copies of what was picked up are dropped at every gateway that holds them. It tries every minute,
and at once when a poll shows the post office answers again. A session the post office refuses
(a forged or revoked credential) is dropped and the log says why; one not reported in 30 days is
dropped too. It remembers for 30 days what it passed on, so that a caller who offers it again is
told *already have* (RP-7 §3.6).

The **Held** box shows it all: **Show** lists each copy (whose, from whom, until when; never the
text), each session waiting to be reported with its last try, and the last reports. On the
command line: `radiopost-gateway held`, and `radiopost-gateway copies --users 50 --days 7
--max-air-bytes 16384` (or `--off`) for the setting.

Forwarding by radio to a partner gateway that can reach the Internet, and handing mail between
two users of a cut-off gateway, come later (Phase 4, wishlist W30 and W31).

## 5. The rules

* **Control.** A gateway left answering with nobody at the radio is under **automatic control**.
  Set Aether's control to Automatic: Aether then transmits only in the segments where automatic
  control is allowed (§97.221(b)) and refuses anywhere else, so register channels in those
  segments. While you are at the radio, local control is yours to choose.
* **Identification.** Aether sends the gateway's Morse identifier at the end of each session.
* **No encryption.** RadioPost signs; it never encrypts (RP-0 §5).
* **Third-party traffic.** The post office marks mail that is third-party traffic, and the gateway
  hands it only to stations in countries that allow it, by the gateway's **Country** (RP-6 §5.1).
  A caller from elsewhere is told in a notice how many such messages wait for them at their post
  office.

## 6. A gateway with no screen

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

`radiopost-gateway serve` answers calls with no panel at all, as before. To update a gateway
run this way: `sudo radiopost-gateway update`, then restart it (`sudo systemctl restart` its unit);
the panel and its log say when a newer release is out.

## 7. What is kept

The gateway's directory is `.radiopost-gateway` in your home directory (`$RADIOPOST_GATEWAY` names
another; Setup shows where):

| File | Holds |
|---|---|
| `gateway.json` | The settings. |
| `key.hex` | The gateway's key. Back it up: a new key needs a new registration. Never share it. |
| `credential.hex` | The gateway's credential, from its registration. |
| `held.json` | Callers' credentials seen, and pieces of uploads a broken link left (RP-3 §7). |
| `store.json` | What it holds for when the Internet is down (§4): copies placed ahead, the sessions carried while cut off and not yet reported, what it passed on, and the post offices' keys and revocation lists. Keep it: a session in it that is lost is mail its sender's RadioPost offers again only after 30 days. |
| `sessions.json` | The last 200 sessions carried, as the Sessions list shows them. |
| `gateway.log` | Everything the log at the bottom of the window says, with the date and time (UTC), from 0.1.0-beta.8 on; also why the connection to Aether ended, and any program fault. Past 2 MB it is kept as `gateway.log.old` and a new one begun. From Aether 0.2.0-beta.150 and RadioPost Gateway 0.1.0-beta.9, Aether's *Send files* carries both. |

## 8. When something is wrong

| You see | Do |
|---|---|
| Waiting for the modem: *cannot reach the modem* | Start Aether. Check the Modem address in Setup. |
| Callers connect to Aether but the gateway never answers them, though it says *Listening* | RadioPost Gateway 0.1.0-beta.6 and earlier lost Aether when Aether restarted (a Setup save) and never claimed its callsign again; Aether before 0.2.0-beta.147 then answered callers itself (from beta.147, under automatic control, it answers nobody while no program holds the callsign, and its header says *not answering*). Update RadioPost Gateway, or stop and start it after saving Aether's Setup. |
| `gateway.log` says, a few times a day on Windows, *the connection to Aether … ended: a read failed: … (os error 997)*, then *waiting for the modem* for a few seconds | RadioPost Gateway 0.1.0-beta.14 and earlier waited for Aether with a read timeout on the connection, which Windows sometimes fails that way; the gateway then lost its callsign at Aether until it claimed it again, about five seconds later. Update RadioPost Gateway. |
| Waiting for the modem: *the modem refused* | Another program holds the callsign at Aether (RMS Trimode, if it uses the same one), or Aether refused it: Aether's log says why. |
| Aether HF stops at start: *host interface: 127.0.0.1:8515 is already in use* | Aether's host programs port was set to 8515, its control port's number. Open Aether's `station.toml` (in PowerShell, `notepad "$env:APPDATA\aether-hf\station.toml"`), write `bind = "127.0.0.1:8300"` under `[host]`, save, and start Aether again. |
| Register: *… .radiopost holds no device key* | The RadioPost app on this computer is not enrolled: enroll it in its Setup, with a code of its own (§1). |
| *This modem does not check the band rules* | Aether's band rules are off, as they are when a host program keys the radio. Set Aether's keying and its rules profile (Aether's Setup steps 1 and 2). |
| A post office *not found* | Its DNS records are not published: name it in Setup, step 3. |
| A post office *refused the gateway* | The credential has run out or was revoked: register again (Setup, step 2). |
| A session ended *nothing heard for 300 seconds* | The path faded or the caller left: the caller's mail is resumed on its next call (RP-3 §7). |
| *polling … for copies: post-office-unreachable* | The Internet or the post office is down: the gateway answers from what it holds (§4), and says when the post office answers again. |
| Held: sessions waiting to be reported, the last try *not reached* | The same: they are reported when the post office can be reached. A last try naming a code (*signature-invalid*) means the post office refused it: the log says so. |

To report a problem, open an issue on [RadioPost's repository](https://github.com/KK4ODA/RadioPost/issues)
with `gateway.log` (§7) and, for a session that failed, Aether's recording of it. From Aether
0.2.0-beta.150, Aether's Log tab → *Send files…* sends both at once.
