# The RadioPost client

`radiopost` is the RadioPost client: the user's device key and credential (RP-1), their mail
(RP-2), and sessions (RP-3) over any modem that has an RPMI adapter (RP-4). It runs on the command
line, as a panel in a browser (`radiopost panel`), or as the desktop application (`app/src-tauri`,
Tauri, design D27), which shows the same panel in a window of its own.

It carries mail **station to station** (RP-3 §10) and through **gateways** (RP-3 §5), and talks
to its post office over the Internet when there is one (RP-5 §6): it enrolls with the one-time
code the post office's operator gives, renews its credential, changes its settings and accept
list, and reports its station-to-station sessions. A credential issued by hand (`radiopost-issue`,
for tests before a post office) is imported with `init`.

## Commands

```
radiopost [--home DIR] COMMAND
```

| Command | Does |
|---|---|
| `key` | Makes this device's key, once, and prints its public key for enrollment. |
| `init --credential FILE [--device-key FILE]` | Keeps the credential a post office issued for this device's key (raw or hex). |
| `trust DOMAIN PUBLIC_HEX [NOT_BEFORE NOT_AFTER]` | Trusts a post office key (RP-1 §7.3: the operator decides). |
| `whoami` | The account and callsign the credential is for. |
| `apply --address ADDR --operator CALL --name NAME --contact EMAIL [--post-office URL]` | Asks the post office for an account from this device (RP-5 §6.7), making its key if there is none, and keeps the application. The post office's operators decide. |
| `application` | Asks how the application stands. Approved, the credential is kept as from an enrollment; sent by postcard, enroll with the card's code. |
| `enroll --address ADDR --operator CALL --code CODE [--post-office URL]` | Enrolls this device with the operator's one-time code (RP-5 §6.1), making its key if there is none, keeps the credential, and remembers the post office. Without `--post-office`, the domain's DNS records name it (RP-1 §7.1); its key is trusted from them too when none is trusted yet. |
| `post-office URL` | The post office's API, when DNS does not name it. |
| `renew` | A new credential for this device (RP-5 §6.2). |
| `settings [--set JSON]` | The account's settings at the post office (RP-5 §6.3); `--set` changes the members it names (`null` removes one). |
| `addresses [--add 'NAME <ADDR>']… [--remove ADDR]…` | The address list: whom you have written to, each with the name written with it and how often, and any added by hand (an address already listed takes the new name). The panel offers it as you type in To and Cc. It is this computer's own; who may email you is the accept list. |
| `accept [--add ENTRY]… [--remove ENTRY]…` | The accept list: who may email this account (RP-5 §6.5, RP-6 §2.2). |
| `report` | Sends the station-to-station sessions waiting to the post office (RP-5 §6.6); each one answered leaves the home. |
| `compose --to ADDR … [--cc ADDR] [--subject TEXT] [--priority 0\|1\|2] [--attach FILE]… [--body FILE]` | Writes a message into the outbox; the text from standard input without `--body`. Each `--to` or `--cc` may hold several addresses, separated by commas, semicolons or spaces, and `Name <address>` as well as a bare address; each is checked first (RP-0 §6.2), and a malformed one is refused with the reason. |
| `import FILE` | Puts a message in its canonical encoding (RP-2) in the outbox. |
| `outbox`, `inbox`, `sent`, `trash` | Lists a folder. |
| `delete ID [--forever]` | Moves a message to the trash from whichever folder holds it; with `--forever`, deletes it for good. Deleting is this computer's own: the post office, gateways and other stations keep what they hold. |
| `restore ID` | Puts a message in the trash back in the folder it was deleted from, where it was: an outbox message keeps its serial and its place. |
| `empty-trash` | Deletes every message in the trash for good. |
| `show ID` | Shows a message; `ID` is the start of its content ID. Attachments too large for the radio say *Held at the post office*. |
| `attachments [ID [NAME]…]` | Asks, on the next call to a gateway, for a received message's held attachments, all or those named (RP-3 §5.6); without `ID`, what will be asked for. They come on the call after, in a new message from the post office (RP-6 §2.5). |
| `reports` | The station-to-station sessions waiting to be reported to the post office (RP-5 §6.6). |
| `call CALLSIGN [--station] [--fetch all\|none\|BYTES]` | Calls a gateway, or with `--station` another client's station. `--fetch` says what to fetch of what waits: everything (the default), what is no larger on the air than BYTES, or only the list; what `fetch` marked comes whatever it says, and messages up to 1 000 bytes come unasked. |
| `gateways [--refresh]` | The gateways your post office lists (RP-5 §4.4): where each is, its channels and hours, nearest first when your grid is set. `--refresh` fetches the list over the Internet, its signature checked with the post office's key; it is kept for when there is no Internet. |
| `grid [SQUARE \| --clear]` | Your Maidenhead grid square (`EM73`, `EM73tt`), to rank gateways by distance. |
| `waiting` | What waited at your post office at the last call to a gateway: every message not yet picked up (RP-3 §6.2). |
| `fetch SERIAL… [--not]` | Fetch these waiting messages on the next call to a gateway, whatever `--fetch` says; `--not` stops asking. |
| `listen` | Answers one station's call. |
| `panel [--port N] [--ui DIR]` | Serves the panel on this computer (port 8620) and says where to open it. |
| `update` | Replaces `radiopost`, and the other RadioPost programs beside it, with the newest release's, once it checks that the project's update key signed them. The desktop application updates itself from its window. |
| `conformance --adapter HOST:PORT`, `conformance off` | Conformance mode (below). |

`call` and `listen` take `--adapter MODEM` and `--bandwidth HZ`. The modem is Aether HF's control
API, `ws://127.0.0.1:8515/v1` as Aether runs it, for which the client runs the Aether adapter (RP-4
§9.1) itself; or an RPMI adapter's `HOST:PORT` (`127.0.0.1:8600` by default, as RP-4 §2 has it). `--plain` sends messages uncompressed; `--clock`, `--nonce`,
`--session-id` and `--challenge` fix the clock and the random draws, so that a session can be
replayed exactly (the conformance tester's §2.4).

**Conformance mode.** `conformance --adapter HOST:PORT` turns it on for this home: `call` and
`listen` then reach only that modem adapter, the conformance tester's, and refuse every other, so
the published test keys the tester issues credentials with never reach the air
([the tester's outline](../../docs/design/01-conformance.md), §2.3). `conformance off` ends it.
The tester runs the client through `core/tester/drivers/radiopost.json`.

For a test on the air between two stations with Aether HF, see
[on-air tests](../../docs/user/on-air-tests.md).

## The panel

`radiopost panel` serves the page in `app/ui/` (plain HTML, CSS and JavaScript, no build step,
light or dark as the system is) and a JSON API under `/api/`, on the loopback interface only, as
the Aether app serves its own. It shows the folders, reads and writes messages with attachments,
asks for held attachments (the reader's *Ask on the next call*), calls a gateway (with a Fetch
setting, the post office's gateway list in *Choose a gateway*, and the *Waiting* folder to mark
what to fetch next) or a station, listens for one, sends the reports, and holds the setup: enrollment with a code
or an application the operators decide (asked after every minute while it waits),
the post office and renewal, the accept list, the device key, a credential imported by hand and
the post offices trusted. When the credential is one of its post office's operators', a
**Post office** button opens the operator's desk (RP-5 §7): the applications waiting, with
**Approve** (a role mailbox if ticked), **Decline** with a reason the applicant reads, and **Send a
postcard** (a code good for 30 days, and a card to print and post to the address on the licence);
the accounts with their devices (each revocable, the computer in use marked); **New code**; the
mail waiting to go out by email; and whether the post office takes applications. Each message row has a trash can, and the reader a **Delete** button (or
the Delete key); the toast that follows has **Undo**. The **Trash** folder says which folder each
message came from, puts it back (**Restore**) or deletes it for good (**Delete forever**, a second
click to confirm), and **Empty trash** empties it. **Addresses** is the address list: everyone a
message is written to is added with the name typed before the address, and To and Cc offer it as
you type (arrows and Enter or Tab to take one, the × to take an address off the list, with
Undo); the browser's own memory of those fields is off, since nobody could see or clear it. The
view finds, adds, renames, writes to and removes addresses, and the reader's *Add to addresses*
adds a sender. While a session runs, a card at the bottom right shows what it has carried: two
rings round an envelope, the outer for what goes out and the inner for what comes in, with the
messages whole, the kilobytes and the time left, and a small pair of rings with the percentage in
the header. The counts come from the session engines (`radiopost::session::Progress`, in bytes of
the messages' air forms), less what the modem still holds of what it was handed (RP-4 `queued`),
so sent mail counts as it goes on the air; the time left comes from what crossed in the last
minute and a half, else the modem's rate (`quality`). A session runs on a thread of its own; while it
runs, the panel changes nothing in the home, so deleting waits for it to end; the address list is
a file of its own and changes at any time. `--ui DIR` serves the page from a directory, for working on it.

Only this computer's own pages reach it: a request must name the panel's own host, so a page that
rebinds its name to 127.0.0.1 is refused, and a change must carry `X-RadioPost: 1`, which another
site's page cannot send without a preflight the panel never grants.

## The desktop application

`app/src-tauri` (`radiopost-app`) starts the panel's server in its own process, on port 8620 or
another free one, and opens a window on it; the window grants the page no access to the
application (no Tauri commands, no capabilities). It uses the same home as `radiopost`. Closing
the window while a call, a session or listening is under way asks first: closing ends it and
leaves the other station waiting. It is a
package of its own, outside the core workspace, because Tauri needs WebKitGTK on Linux:

```
sudo apt-get install libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libxdo-dev libayatana-appindicator3-dev
cd app/src-tauri && cargo run
```

Its icons (PNGs, the Windows ICO, the macOS ICNS and the panel's `icon.svg`) are drawn by
`tools/figures/icons.py`; run it after changing the drawing, and CI checks them.

## Two stations on one machine

`rpmi-loopback` (in `core/rpmi`) is a simulated channel whose stations are reached over TCP. With
the test identities of `vectors/` (their keys are published, so they are never real):

```
rpmi-loopback --station KK4ODA@127.0.0.1:8600 --station N0CALL@127.0.0.1:8601 &
radiopost --home facundo init --device-key facundo.key --credential facundo.cred
radiopost --home ana init --device-key ana.key --credential ana.cred
radiopost --home facundo trust radiopost.org <key> ; radiopost --home ana trust radiopost.org <key>
…                                                     # and hampost.org's key, for each
echo "All well here." | radiopost --home facundo compose --to ana@hampost.org --subject Hola
radiopost --home ana listen --adapter 127.0.0.1:8601 &
radiopost --home facundo call N0CALL --station --adapter 127.0.0.1:8600
radiopost --home ana inbox
```

On the air, the adapter is the modem's: `radiopost-aether` for Aether HF (`adapters/aether`).

## Updates

Every program carries the release's version (`tools/release.py`). The panel asks the update
channel (`channel-beta` on the releases page) when it starts and every six hours, and a line at the
top says when a newer release is out. In the desktop application, **Update and restart** installs it
with Tauri's updater; served by `radiopost panel` in a browser, **Download** opens the release's
page. Either way a file is taken only when the project's update key signed it (`core/update`).

## Renewal

A credential lasts 90 days, and one that has run out cannot be renewed: the device enrolls again
with a new code (RP-5 §6.2). So in its last month (`renewal::WITHIN`, 30 days) the client renews it
by itself, by whichever path is open first. The panel asks over the Internet when it starts and
every hour, between sessions; the badge reads *Renewing credential* while it does, and the log
says what came of it, a failure once until it changes. A call to a gateway asks for one in
`HELLO` (RP-3 §5.1, §5.8), from the panel or `radiopost call`, and keeps what the gateway brings.
`radiopost renew` and Setup's **Renew credential** renew at once. A gateway renews its own
credential the same way, while it answers ([the gateway's guide](../../docs/user/gateway.md)).

## What is kept

The home (`--home`, else `$RADIOPOST_HOME`, else `.radiopost` in the user's home directory) holds
the device seed, the credential, the trusted keys, each message's canonical encoding named by its
content ID, partial messages to resume (RP-3 §7), the address list (`addresses.json`) and `state.json`: the folders and the trash, the next serial,
the station called's replay cache (RP-1 §6.3), what each gateway holds, the reports, each received
message's serial at the post office, the held attachments to ask for, the post office's gateway
list and your grid square, what waited at the last call to a gateway, and what to fetch next. A request leaves the home
once a gateway has answered it; a link broken before that keeps it for the next call. Files are
written whole or not at all, and a message received is on disk before its `RECEIPT` goes out (RP-3
§5.4). A message deleted for good leaves the home once no folder holds it, and what was asked for
its held attachments is no longer asked.

A message a station takes moves to `sent`. Its other recipients get it when the session is
reported to the post office (D30, RP-5 §6.6), not over the air again; each session's report, with
both stations' proofs, waits in `state.json` until `report` (or the panel's Send reports) sends it.

## Tests

`tests/panel.rs`: the panel's guards, a message written with an attachment and carried by a call
made from the panel (with what it carried, as the progress card shows it), a held attachment asked for, listening stopped, and the trash (deleted,
restored to its place, deleted for good, emptied, and refused while a session runs), and the
address list (learned from a message with the names typed, added to and renamed, taken off and
put back as it was, during a session too). `tests/two_clients.rs`: two clients replay session-03
byte for byte through the loopback adapter
with the vector's clocks and draws, and end with the folders the session says and the reports
`vectors/direct.json` has; the program itself sets up two homes, composes, listens and calls; and it
refuses a credential for another device's key. `tests/online.rs`: against a post office service
over HTTP, two users enroll with the program (a wrong code and a used code refused), change a
setting and the accept list and renew, carry a station-to-station session, and report it: the
other station's copy filed as picked up, the email copy waiting to go out; a device enrolls
from the panel; and two applicants join from their panels, one approved from the operator's desk,
one sent a code by post.
