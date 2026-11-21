# Running a gateway with RadioPost Gateway

How a licensed operator sets up a RadioPost gateway on the computer at their radio, registers it
with their post office, and leaves it answering. Users call the gateway by radio; it carries each
session to the user's own post office over the Internet (RP-3 §9, RP-5 §5). When the Internet is
down, it goes on: it checks callers itself, hands them the copies of their mail it holds, keeps
what they send, and reports it all when the Internet is back, or hands it by radio to a partner
gateway that can reach the Internet (§4).

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
  * For station-to-station mail with you, other operators call your own callsign, not the
    gateway's, while your RadioPost client listens under it (**Listen for P2P**) beside the gateway: one
    Aether serves both. A client that calls the gateway's callsign for station-to-station mail is
    welcomed as by any gateway, hangs up, and the gateway's log says why.
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
  through it. With several accounts in it (its callsigns, each with its own post office), you
  choose which one the gateway registers with. It needs nothing else: you can keep using RadioPost
  on another computer. Each
  computer enrolls with a code of its own, since each has its own key: an enrollment on another
  computer does not count here, and copying its `.radiopost` folder would put one key on two
  computers.

## 2. Setup

Open RadioPost Gateway. Until it is set up and registered, it says what is missing; click **Set up
the gateway**.

**1. The gateway.** Enter its **Callsign**: yours with an SSID, such as `KK4ODA-1`. Leave
**Modem** at `ws://127.0.0.1:8515/v1`, where Aether listens on the same computer. **Country** is
where the gateway operates, two letters: it decides which third-party mail the gateway may hand
over (RP-6 §5.1). Leave *The modem does not check the band rules* clear: Aether checks them. With
a modem that does not, but says where the radio's dial is (RP-4 §4.1), the gateway checks the dial
itself and answers only inside an automatic-control segment; the badge reads **Not answering
here** while the radio is outside one. Keep
*Answer calls when the app opens* ticked for a gateway left on its own. Tick *Hold copies of
users' mail, for when the Internet is down* for a gateway with room to spare (§4), and say for how
many users, how many days, and the largest message it holds with its text. Click **Save**. The
gateway makes its key; it is shown under the form. Change the copies setting later, and register
again (step 2) so that the post office knows.

**More modems** (wishlist W26) lets one gateway answer on several modems: Aether and Mercury on one
radio, or Aether on HF and packet on VHF. Give each modem's RPMI adapter (`radiopost-mercury`,
`radiopost-vara`, from the tools archive), one a line, as `host:port`. The gateway carries one
session at a time: while it carries one, it tells the other modems not to answer, and a call one of
them answered before it heard is ended (`abort`) once the session is over. Calls to partners
(RP-7 §2) go on the first **Modem**. Two modems on one radio must share its keying through rigctld
or FLRig, since only one program can hold a CAT port; each hears the other's signal as busy.

**2. Registration.** **RadioPost app's home** is the folder where the RadioPost app on this
computer keeps its keys and credentials (`.radiopost` in your user folder): leave it. **Account**
lists that app's accounts, each callsign and address with its post office in brackets; it starts
on the one the app has chosen. Pick the one the gateway registers with: that account's post office
lists the gateway, and the gateway's credential names that address. The line under it says, before
you click, where the gateway will be registered and as whom, and the button names the post office:
**Register with radiopost.org**. Under **Channels**, give each dial frequency (MHz, upper sideband) where callers will find the
gateway, the **Modem** callers use there (RP-0 §9.4's names), and its bandwidth (for Aether, 500
or 2300 Hz; on an FM radio, the channel and its width, RP-5 §4.4). Tick **Scan these channels** to listen on all of them with one radio
(wishlist W50, RP-4 §4.2): Aether 0.2.0-beta.162 or later moves the radio over them, a while on
each, the first being home, and never leaves one while a call is arriving or anything is owed there.
Two to four channels, all for the first **Modem**, on a radio Aether can tune. The record then
carries each scanned channel's `scan_s` (RP-5 §4.4), how long a caller's call must keep trying; the
gateway registers again by itself, from the same app's home, whenever its modem says another. List
only channels the gateway listens on. Tuning the radio yourself ends the scan, and the gateway does
not start it again until it is stopped and started; a call to a partner stops it for the call and
starts it again after. When the scan ends, or Aether drops a channel the radio will not take, the
gateway registers again with what it still listens on: once the scan has ended, the channel the
radio stays on alone, with no `scan_s`. When the scan runs again, it registers all its channels
again. **Grid** and **Place** help callers choose
a gateway near them; **Hours** says when it answers (`always`, or in words). **Listening times,
UTC** says the same for callers' programs, so their lists can show whether the gateway listens now:
each day's times such as `18:00-02:00`, several with commas, or empty if it listens always (RP-5
§4.4 `listens`; `--listens` on the command line). Your post office adds when the gateway last
reached it, to the minute, so callers can tell a gateway whose computer has been off for days
(`seen`). Click **Register**.
Your post office lists the gateway for its users and issues it a credential, valid for 90 days.
While the gateway answers, it renews that credential by itself in its last month, with your
account's post office, and uses the new one from its next session; the log says so. If renewing
keeps failing (the log says why, once), register again before the credential runs out (the same
way: it replaces the registration); the Registration box turns red two weeks ahead.

Once registered, the top of step 2 says where: *KK4ODA-1 is registered with radiopost.org's post
office (https://po.radiopost.org/rp/v1), from KK4ODA · kk4oda@radiopost.org.* The form keeps the
channels, grid, place and hours you registered, so registering again changes only what you change.

**Unregistering.** **Unregister**, beside that line, clicked twice, withdraws the registration
(RP-5 §6.4): logged in as the account it was registered from, the gateway asks its post office to
take it off its gateway list. The post office revokes the gateway's credential and stops placing
copies of users' mail there; the copies the gateway holds are dropped, and the mail itself stays in
the users' mailboxes. Stop the gateway first. While sessions it carried with the Internet down
still wait to be reported (Held), it refuses: they hold users' mail, which could never be handed in
with the credential revoked. Start it with the Internet up, let it report them, then unregister.

A gateway is registered with one post office at a time: registering with another account's post
office while it is registered is refused. **To move it**, say from radiopost.org to your own post
office: **Unregister**, pick the other account under **Account**, and **Register with …**. A post
office that does not take withdrawals yet (one running RP-5 before 1.2.0) keeps the gateway on its
list: the gateway says so and keeps its registration, and that post office's operators remove it
(on their desk, **Gateways**, or `radiopost-po unregister`). They can also remove a gateway
registered by mistake without its operator.

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

**Refused** lists the callers you refuse, and **Show** lets you add one. Name an account by its
address (`ana@example.org`) or a station by its callsign: a callsign without an SSID refuses every
SSID of it (`N0CALL` refuses `N0CALL-5`), and `N0CALL-5` refuses that station alone. The note is
for you; the caller is told only that this gateway's operator does not carry its sessions
(`refused-by-policy`). It applies from the caller's next call, whether the gateway is answering or
not, and nothing of it reaches the post office. The Sessions list shows such a call as refused, and
**Carry again** takes a caller off the list. Do it when a station misuses the gateway, such as mail
the rules do not allow on the air: the operator who learns of it must stop it. A whole post
office is refused from the command line (§6).

**Stop** ends a session under way and lets the modem go. Settings change only while the gateway is
stopped. Closing the window stops the gateway too. Stopped, it closes at once; otherwise it asks
first, and says what closing stops:

| Badge | Closing |
|---|---|
| Waiting for the modem | Stops the wait: the gateway was not listening for calls yet. |
| Listening | Stops listening: calls to the gateway's callsign go unanswered until it runs again. |
| Session | Ends the session with the station named, and the gateway stops answering. |

**Unattended.** With *Answer calls when the app opens* ticked, the gateway answers as soon as the
app opens. To have it open when you sign in: on Windows, put a shortcut to RadioPost Gateway in the
Startup folder (Win+R, `shell:startup`); on macOS, add it to Login Items (System Settings,
General); on Linux, to your desktop's startup applications.

**Arranging the windows.** RadioPost Gateway, the RadioPost client and Aether each open where you
left them, at the size you left them, so arrange them once. One layout that fits a single screen:

- RadioPost Gateway on the left, as tall as the screen.
- Aether in **Compact** at the top right: the state, the four readings and the two plots. Stretch
  it to the width you want.
- The RadioPost client under Aether, as wide as Aether.

Aether keeps Compact's size separate from Full's. A window left on a screen that is no longer
connected opens where the system puts it.

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
* held attachments cannot be asked for: the notice says so;
* **mail between its own users is handed over at once** (RP-7 §4.4). Its own users, here, are
  those it received a copy placed ahead for in the last 30 days. When one sends a message to
  another, the gateway keeps it for the other too, and offers it at their next call, cut off or
  not yet reported, without waiting for their post office. Their RadioPost takes it as any other
  message; its pickup is reported with their session, and their post office then marks it picked
  up, so it is not offered to them again.

**When the Internet is back** (RP-5 §5.6). The gateway reports each session it carried while cut
off to the caller's post office, oldest first: the messages go on, the pickups are marked (those
of mail handed over here too), and the copies of what was picked up are dropped at every gateway
that holds them. What it kept for its own users goes once the message's session is reported: from
then on it comes back as copies placed ahead. It tries every minute,
and at once when a poll shows the post office answers again. A session the post office refuses
(a forged or revoked credential) is dropped and the log says why; one not reported in 30 days is
dropped too. It remembers for 30 days what it passed on, so that a caller who offers it again is
told *already have* (RP-7 §3.6).

The **Held** box shows it all: **Show** lists each copy (whose, from whom, until when; never the
text), each session waiting to be reported with its last try, and the last reports. On the
command line: `radiopost-gateway held`, and `radiopost-gateway copies --users 50 --days 7
--max-air-bytes 16384` (or `--off`) for the setting.

**Forwarding by radio to a partner** (RP-7 §2, §3). A partner is another gateway, named by its
callsign, that this one can reach by radio. While this gateway is cut off, it can hand its queue
to a partner that reaches the Internet, so the mail does not wait for the Internet to come back
here.

* **Name each other.** In the **Partners** box, **Show**, type the other gateway's callsign and
  **Add** (stop the gateway first: it holds its settings while it answers). Its operator does the
  same with yours. A gateway answers forwarding calls from its partners only, and refuses others.
* **Forward now.** While the gateway answers, **Forward now** beside a partner calls it between
  sessions, on the gateway's modem and channel. Both gateways must be on the same channel: tuning
  the radio to the partner's channel comes in a later version. The call carries every session
  waiting to be reported, with the messages sent in it, each with its user's own signature, so
  the partner can check it. The partner keeps each one with both callsigns on it and reports it
  to its post office, or, cut off too, forwards it to its own partners in turn. This gateway then
  lets the session go, and remembers its messages as passed on.
* **Mail coming back.** When the post office has a route naming the partner for this gateway, it
  sends the mail for this gateway's users to the partner while this gateway is cut off: the
  copies this gateway never confirmed, and everything that arrives after (RP-7 §4.1). The partner
  hands it over in the same call, each message signed by the post office for this gateway. This
  gateway checks the signature and keeps the message as a copy placed ahead, as if the post
  office had copied it here itself, and its users take it as any other.
* **Holding mail for a partner.** The other way round, a gateway the post office names as a
  route's *via* holds mail for its cut-off partner until the partner calls. It must hold copies
  (Setup, step 1), so that it polls the post office for them. Once the partner's Internet is back,
  the post office takes back what the partner did not collect.
* **What the box says.** Each partner shows how many sessions wait for it and how the last call
  went. The **Held** list shows a session that came from a partner with *via* and the partner's
  callsign.

* **Your windows** (RP-7 §5). A window is when partners may call this gateway: a channel, and a
  time that comes round every hour or every few hours. In the **Partners** view, under
  **Windows**, give the dial in MHz (upper sideband), Aether's bandwidth, how often, when it opens
  (UTC; every hour, only the minutes count) and for how many minutes, and **Add** (the gateway
  stopped). A window is accepted only when Aether's signal on it lies inside one segment where
  the rules let an unattended station start a data transmission (RP-7 §5.1; in the United States
  §97.221(b): 80 m 3.585–3.600 MHz, 40 m 7.100–7.105, 30 m 10.140–10.150, 20 m 14.0950–14.0995 and
  14.1005–14.1120, 17 m 18.105–18.110, 15 m 21.090–21.100, 12 m 24.925–24.930, 10 m 28.120–28.189,
  and 6 m and up). On 40 m that leaves dials from 7.0998 to 7.1022 MHz for the 2 300 Hz signal.
  Partners read your windows from your record at your post office: **register again** in Setup
  after a change. During a window, leave the radio on its channel.
* **Calling by itself.** While the gateway answers and cannot reach its own post office, it calls
  each partner inside that partner's windows: once, then, if nobody answers, twice more a minute
  apart, then it waits for the next window. It never calls while Aether reports the channel busy.
  It reads its partners' windows from the gateway lists of the post offices it knows, fetched
  every six hours while it has partners and kept for when the Internet is down; a window a
  partner published after the last fetch is not known until the next. The Partners list shows
  each partner's next window. When the modem can set the radio's dial (Aether keying over CAT,
  rigctld or FLRig), the gateway tunes the radio to the partner's window and back after the call,
  and says so in its log. Otherwise a partner's window must be on the channel this gateway's
  radio is on.
* **Which partner gets the queue** (RP-7 §6.2). The gateway remembers whether each partner
  reached the Internet at its last call. In the window of a partner that did not, it hands that
  partner its queue only if no partner that did opens a window within the hour; otherwise it
  calls only to collect its users' mail, and keeps the queue for the better partner. A partner
  it never called counts as neither.

On the command line: `radiopost-gateway partner --add N1CALL-10` (or `--remove`),
`radiopost-gateway partners`, and `radiopost-gateway forward --to N1CALL-10`, which opens the
modem, calls once and closes it, for a gateway that is not answering calls (an operator at the
radio may call a partner at any time). `radiopost-gateway window --dial 7.101 --bandwidth 2300
--every 3600 --at 900 --length 600` adds a window opening at a quarter past every hour for ten
minutes, `windows` lists them with whether each may be published, and `window --remove 1` takes
the first off.

## 5. The rules

* **Control.** A gateway left answering with nobody at the radio is under **automatic control**.
  Set Aether's control to Automatic: Aether then transmits only in the segments where automatic
  control is allowed (§97.221(b)) and refuses anywhere else, so register channels in those
  segments, and windows too (§4). While you are at the radio, local control is yours to choose.
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

`radiopost-gateway accounts` lists the RadioPost app's accounts (its home is `--home`, else
`.radiopost` in your home directory) and where the gateway is registered now. `radiopost-gateway
register --account kk4oda@radiopost.org --channel 7101000,USB,aether,2300 --grid EM73 --place
"Atlanta, GA"` registers with that account's post office (`--account` takes the address, the
callsign or the number `accounts` gives, and is needed only when the app holds more than one).
`radiopost-gateway unregister` withdraws the registration; stop the gateway first. `unregister
--here` forgets it on this computer only, without asking the post office: for a post office that
cannot be reached any more, whose operators then remove the record themselves.

`radiopost-gateway serve` answers calls with no panel at all, as before. `radiopost-gateway modem
--add 127.0.0.1:8601` has it answer on one more modem, `modem --remove` no longer, and `modems`
lists them; stop the gateway first. `radiopost-gateway block
--caller N0CALL --note "why"` refuses a caller, `unblock --caller N0CALL` carries it again, and
`blocked` lists the callers and post offices refused; a running gateway takes the change at the
next call. `block --domain example.org` refuses a whole post office (RP-1 §7.3) and `unblock
--domain` lifts it; that one is a setting, so stop the gateway first. To update a gateway
run this way: `sudo radiopost-gateway update`, then restart it (`sudo systemctl restart` its unit);
the panel and its log say when a newer release is out.

## 7. What is kept

The gateway's directory is `.radiopost-gateway` in your home directory (`$RADIOPOST_GATEWAY` names
another; Setup shows where):

| File | Holds |
|---|---|
| `gateway.json` | The settings. |
| `key.hex` | The gateway's key. Back it up: a new key needs a new registration. Never share it. |
| `credential.hex` | The gateway's credential, from its registration. Gone once it is unregistered. |
| `record.json` | What was last registered (all your channels, grid, place, hours) and from which account, app home and post office. After **Unregister**, the record alone, and where it was withdrawn: the form fills from it. |
| `held.json` | Callers' credentials seen, and pieces of uploads a broken link left (RP-3 §7). |
| `store.json` | What it holds for when the Internet is down (§4): copies placed ahead, the sessions carried while cut off and not yet reported (its own, and those a partner forwarded), what it passed on, the post offices' keys and revocation lists, and what it knows of each partner. Keep it: a session in it that is lost is mail its sender's RadioPost offers again only after 30 days. |
| `sessions.json` | The last 200 sessions carried, as the Sessions list shows them. |
| `refused.json` | The callers you refuse, each with your note and since when. |
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
| Register: *KK4ODA-1 is registered with radiopost.org's post office now …* | You picked an account at another post office. Unregister the gateway first (§2), then register it with the account you want. |
| Unregister: *… still wait to be reported* | Sessions carried while the Internet was down hold users' mail. Start the gateway with the Internet up: it reports them, and Held says when. Then unregister. |
| Unregister: *… keeps KK4ODA-1 on its gateway list* | That post office does not take withdrawals yet. Ask its operators to remove the gateway, then click **Unregister** again: the gateway finds itself off the list and forgets the registration. |
| Unregister: *… is not in the RadioPost app on this computer* | The account the gateway was registered from is not enrolled here. Enroll it in RadioPost, or ask its post office's operators to remove the gateway and use `radiopost-gateway unregister --here`. |
| *This modem neither checks the band rules nor says where the radio's dial is* | Aether's band rules are off, as they are when a host program keys the radio, and it cannot read the dial. Set Aether's keying and its rules profile (Aether's Setup steps 1 and 2). |
| **Not answering here**: *the radio is at … MHz … outside the segments* | The modem checks no band rules, so the gateway judges the dial: turn the radio to the gateway's channel. |
| A post office *not found* | Its DNS records are not published: name it in Setup, step 3. |
| A post office *refused the gateway* | The credential has run out or was revoked: register again (Setup, step 2). |
| A session ended *nothing heard for 300 seconds* | The path faded or the caller left: the caller's mail is resumed on its next call (RP-3 §7). |
| *polling … for copies: post-office-unreachable* | The Internet or the post office is down: the gateway answers from what it holds (§4), and says when the post office answers again. |
| Held: sessions waiting to be reported, the last try *not reached* | The same: they are reported when the post office can be reached. A last try naming a code (*signature-invalid*) means the post office refused it: the log says so. |

To report a problem, open an issue on [RadioPost's repository](https://github.com/KK4ODA/RadioPost/issues)
with `gateway.log` (§7) and, for a session that failed, Aether's recording of it. From Aether
0.2.0-beta.150, Aether's Log tab → *Send files…* sends both at once.
