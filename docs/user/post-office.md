# Running a post office

How to run a RadioPost post office for a domain: the RadioPost service (`radiopost-po`) beside the
Stalwart mail server and the Caddy web server, on one machine, as the pilot (`radiopost.org`,
design D29) runs. The service speaks RP-5 to gateways and clients, and RP-6 to the mail server: it
decides at the door which email comes in, makes each email a RadioPost message, and sends the mail
users write by radio out as email.

The pilot runs on a Hostinger KVM 1 virtual server (one core, 4 GB) with Debian 13, Stalwart 1.1
and Caddy 2.11. The steps below are the ones that set it up, in order.

## The short way

**Setting up.** On a fresh Debian server, as root, the installer does §3 to §7 itself, step by
step, and says what you do by hand (the mail server's setup page, the DNS records, the reverse DNS)
with the values to type:

```
curl -LO https://github.com/KK4ODA/RadioPost/releases/download/v0.1.0-beta.11/radiopost-tools-linux-x64.zip
unzip radiopost-tools-linux-x64.zip
./radiopost-tools-linux-x64/radiopost-po install --domain radiopost.org --operator KK4ODA --contact postmaster@radiopost.org
```

`--dry-run` says everything it would do and does nothing. Each step checks whether it is done
already, so running it again is safe and finishes what an earlier run did not. The secrets it makes
(the door's token, the mail server's `radiopost` password) are kept in `/root/radiopost-install.json`.

**Every day.** The post office is run from the RadioPost client. Enroll your own client as the
callsign the post office names as its operator (`--operator`), and a **Post office** button
appears in its window: the desk (§12). Your credential is the login: there is no password and no
administration page on the Internet. The commands below stay for setting up and for what goes
wrong.

---

## 1. What runs where

| Program | Ports | Does |
|---|---|---|
| Caddy | 80, 443 | Gets and renews the certificate for `po.<domain>`, sends `https://po.<domain>/rp/v1/…` to `radiopost-po` and everything else to Stalwart's web listener. |
| Stalwart | 25 (SMTP in and out), 465 (mail programs, implicit TLS), 993 (IMAP); its web interface on `127.0.0.1:8080`, behind Caddy | Receives and sends email, stores the mailboxes users read in ordinary mail programs, signs outgoing mail (DKIM). |
| `radiopost-po` | 8770 (the API, behind Caddy), 8771 (the operator's commands), 8772 (the door): all on `127.0.0.1` | Accounts, credentials, sessions from gateways, the radio mailboxes, the door, email in and out. |

Mail between the service and Stalwart stays on the machine: Stalwart calls the door at
`http://127.0.0.1:8772/` during each SMTP conversation, and the service submits outgoing email to
Stalwart on port 465 as the account `radiopost`.

Stalwart 1.1 opens POP3 (995) and ManageSieve (4190) as well; the firewall keeps them closed.

## 2. What the server needs

These decide which provider can host it:

* **Port 25 open both ways.** Many providers block outbound port 25 for new accounts, some lift it
  on request, some never. Ask before choosing. Hostinger does not block it on its virtual servers,
  but limits each to 5 emails a minute (§5.6 keeps under that).
* **Reverse DNS** that the operator sets: each of the server's addresses must name the mail host
  (`po.radiopost.org`), and that name must resolve back to the address (RP-6 §1). IPv6 too, if the
  server has an address there: Gmail is strict about mail over IPv6.
* **A fixed IPv4 address** whose reputation is clean: check it on Spamhaus and MXToolbox before
  anything else. A provider cannot get an address off an outside block list.
* **A small machine** is enough for a pilot with a handful of users.
* Linux, with `systemd`. The release's tools archive has `radiopost-po` for Linux (x86-64).

## 3. The server

On a fresh Debian server, as root:

```
apt update && apt full-upgrade -y
hostnamectl set-hostname po.radiopost.org
sed -i 's/^127\.0\.1\.1.*/127.0.1.1\tpo.radiopost.org po/' /etc/hosts
```

Hostinger's image runs cloud-init, which would put its own name back at a reboot:

```
printf 'preserve_hostname: true\nmanage_etc_hosts: false\n' > /etc/cloud/cloud.cfg.d/99-radiopost.cfg
```

Check outbound port 25 (`open` is the answer wanted):

```
timeout 10 bash -c '</dev/tcp/gmail-smtp-in.l.google.com/25' && echo open
```

Log in with an SSH key, and then turn password logins off:

```
printf 'PasswordAuthentication no\nPermitRootLogin prohibit-password\n' > /etc/ssh/sshd_config.d/00-radiopost.conf
sshd -t && systemctl reload ssh
```

The `00-` matters: a file the provider put in that folder may say otherwise, and the first setting
read wins. The firewall, and Debian's security updates:

```
apt install -y ufw unattended-upgrades
ufw default deny incoming
ufw default allow outgoing
for p in 22 25 80 443 465 993; do ufw allow $p/tcp; done
ufw enable
```

At the domain's DNS host, publish `po.<domain>` A (and AAAA) to the server's addresses; then set
the reverse DNS of each address to `po.<domain>` at the provider (in Hostinger's hPanel: VPS,
Settings, IP address, Set PTR record). The domain keeps its null MX, SPF and DMARC records until
§7.

## 4. Caddy and the certificate

Install Caddy from its own repository:

```
apt install -y curl gnupg
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' > /etc/apt/sources.list.d/caddy-stable.list
chmod o+r /usr/share/keyrings/caddy-stable-archive-keyring.gpg /etc/apt/sources.list.d/caddy-stable.list
apt update && apt install -y caddy
```

`/etc/caddy/Caddyfile` (`issuer acme` keeps the certificate with Let's Encrypt, so its folder is
known):

```
po.radiopost.org {
    tls {
        issuer acme
    }
    handle /rp/v1/* {
        reverse_proxy 127.0.0.1:8770
    }
    handle {
        reverse_proxy 127.0.0.1:8080
    }
}
```

`systemctl reload caddy`, and the certificate appears under
`/var/lib/caddy/.local/share/caddy/certificates/acme-v02.api.letsencrypt.org-directory/po.radiopost.org/`.

Stalwart needs the same certificate for its mail ports. `/usr/local/sbin/stalwart-certs` copies it
when it has changed and restarts Stalwart:

```sh
#!/bin/sh
set -e
src=/var/lib/caddy/.local/share/caddy/certificates/acme-v02.api.letsencrypt.org-directory/po.radiopost.org
dst=/etc/stalwart/tls
mkdir -p "$dst"
cmp -s "$src/po.radiopost.org.crt" "$dst/po.radiopost.org.crt" && exit 0
install -o stalwart -g stalwart -m 0640 "$src/po.radiopost.org.crt" "$dst/"
install -o stalwart -g stalwart -m 0600 "$src/po.radiopost.org.key" "$dst/"
systemctl try-restart stalwart
```

A `stalwart-certs.service` (`Type=oneshot`, `ExecStart=/usr/local/sbin/stalwart-certs`) and a
`stalwart-certs.timer` (`OnCalendar=daily`, `RandomizedDelaySec=1h`, `Persistent=true`) run it
every day; Caddy renews weeks before a certificate ends. Run the service once by hand after
Stalwart is installed.

## 5. Stalwart

### 5.1 Install

Stop nothing yet: Caddy holds port 443 first, and Stalwart, which would take it, starts without it
(a listener that cannot bind is skipped, and the rest start).

```
curl --proto '=https' --tlsv1.2 -sSf https://get.stalw.art/install.sh -o install.sh
sh install.sh
journalctl -u stalwart -n 200 | grep -A8 'bootstrap mode'
```

The last command shows the temporary `admin` password. The setup wizard listens on port 8080,
which the firewall keeps closed: reach it through an SSH tunnel
(`ssh -L 8080:127.0.0.1:8080 root@<server>`) at `http://localhost:8080/admin`.

### 5.2 The wizard

| Screen | Set |
|---|---|
| 1 | Server Hostname `po.radiopost.org`; Default Email Domain `radiopost.org`; **untick** Automatically Obtain TLS Certificate (Caddy holds port 443); keep Generate Email Signing Keys. |
| 2 Storage | The defaults (RocksDB in `/var/lib/stalwart/`). |
| 3 Account directory | Use the internal directory. |
| 4 Logging | The default (a log file in `/var/log/stalwart/`). |
| 5 DNS | Manual DNS server management. An automatic one would keep a key that can change every domain of the DNS account on the server; the price of manual is that DKIM keys are not rotated. |

Keep the `admin@<domain>` password the last screen shows. Then `systemctl start stalwart-certs`
and `systemctl restart stalwart`, and sign in at `https://po.<domain>/admin`.

### 5.3 Listeners, certificate and client addresses

In the administration (the search box at the top finds each page by name):

1. **Certificates**: a new one, certificate of type File `/etc/stalwart/tls/po.radiopost.org.crt`,
   private key of type File `/etc/stalwart/tls/po.radiopost.org.key`. It covers the hostname, so
   it is the default certificate too.
2. **Listeners**: delete `https`; change the bind of `http` to `127.0.0.1:8080`, so only Caddy
   reaches it.
3. **HTTP, General**: Client IP source X-Forwarded-For, forwarded hops 1. Without it every visitor
   seems to come from Caddy, and one person guessing passwords gets everyone banned.

Then on the server:

```
echo 'STALWART_PUBLIC_URL=https://po.radiopost.org' >> /etc/stalwart/stalwart.env
systemctl restart stalwart
grep -h -E 'listen-start|build-error|ERROR' /var/log/stalwart/* | tail -n 20
```

The listeners should be 25, 465, 993, 995, 4190 and `127.0.0.1:8080`, nothing on 443, no errors.

### 5.4 Sender authentication

**Sender Authentication**: DMARC verify `strict` on port 25 (the default says `relaxed`). Stalwart
then refuses mail failing DMARC for a domain that publishes `p=reject`, for every mailbox it holds.
The door makes RP-6 §2.1's full check for the RadioPost accounts itself (§5.7).

### 5.5 The DKIM keys

**Management, Domains**, the menu next to the domain, **View Zone File**: Stalwart made an RSA key
(`v1-rsa-…`), which RP-6 §1 asks for, and an Ed25519 one. Both records go into DNS in §7.

### 5.6 Hostinger's limit

**Rates & Quotas, Outbound Rate Limits**: a new one, no key (one count for all mail), rate 4 per
60000 ms. Mail over it waits in the queue; nothing is refused.

### 5.7 The post office's account and the door

1. **An account `radiopost`** (Management, Directory, Accounts), with a password of letters and
   digits only, since it goes in a URL: `openssl rand -hex 24` makes one. The service submits each
   user's mail under that user's address, which Stalwart allows only an account's own addresses
   unless told otherwise: in **AUTH Stage**, give Must Match Sender a rule, `false` if
   `authenticated_as == 'radiopost' || authenticated_as == 'radiopost@radiopost.org'`, otherwise
   `true`. `authenticated_as` is the account's name, which on the pilot's Stalwart 1.1 is its full
   address even when the service signs in as plain `radiopost`; a rule naming only one form
   leaves every user's mail refused (`501 5.5.4 You are not allowed to send from this
   address`), and `check` says so. Restart Stalwart after saving.
   If Stalwart refuses the service's login as `radiopost`, give the full address instead, written
   `radiopost%40radiopost.org` in the URL.
2. **The MTA hook** (Settings, MTA, Filters, MTA Hooks): URL `http://127.0.0.1:8772/`; stages
   `rcpt` and `data`; HTTP authentication Bearer with the hook token (`openssl rand -hex 24`, the
   same one given to `init --hook-token`); timeout 30000 ms (the door's own checks ask DNS); keep
   *temporary failure on error*, so that a stopped service delays mail rather than letting it all
   through; Enable `true` if `local_port == 25`, otherwise `false` (as JSON,
   `{"match": {"0": {"if": "local_port == 25", "then": "true"}}, "else": "false"}`), so the door
   judges mail from the Internet, not what a user's own mail program sends.
3. **An account for each user**, the same address as their RadioPost account. Stalwart refuses a
   recipient it has no mailbox for right after the door's `RCPT TO`, so without it no mail reaches
   the user at all. Users can read it in an ordinary mail program too (IMAP 993, SMTP 465).

What the door reads (RP-6 §2.1): Stalwart 1.1 calls its hooks with the message as the client sent
it, `Authentication-Results` and all, and with no verdicts of its own. The door never believes a
message's own `Authentication-Results`; it checks SPF, DKIM and DMARC itself, from the client's
address and EHLO name in the request, asking the system's DNS resolver. A check that cannot finish
answers `451 4.7.1`, and the sending server tries again. Addresses that are no RadioPost account
(`postmaster@`, `abuse@`) are Stalwart's: the door lets them through.

## 6. The post office

```
apt install -y unzip
curl -LO https://github.com/KK4ODA/RadioPost/releases/download/v0.1.0-beta.11/radiopost-tools-linux-x64.zip
unzip radiopost-tools-linux-x64.zip
install -m 0755 radiopost-tools-linux-x64/radiopost-po /usr/local/bin/
useradd --system --home-dir /var/lib/radiopost --create-home --shell /usr/sbin/nologin radiopost
runuser -u radiopost -- radiopost-po --dir /var/lib/radiopost init --domain radiopost.org \
    --base-url https://po.radiopost.org/rp/v1 --operator KK4ODA \
    --contact postmaster@radiopost.org --hook-token HOOK_TOKEN \
    --smtp 'smtps://radiopost:PASSWORD@po.radiopost.org:465'
```

It makes the post office's key (`key.hex`: keep it, back it up off the machine, never share it),
its settings (`config.json`) and an empty state, and prints:

* the DNS records to publish: the `_radiopost` TXT records (RP-1 §7.1);
* the line each user's client trusts it with.

`config.json` holds what `init` was given. Its email settings:

| Member | Default | Meaning |
|---|---|---|
| `hooks` | `127.0.0.1:8772` | Where the door listens for Stalwart's MTA hooks. Off the loopback interface it needs `hook_token`. |
| `hook_token` | none | The bearer token Stalwart sends with each hook (`--hook-token`). |
| `smtp` | none | Stalwart's submission port as a URL: `smtps://` (implicit TLS, port 465), or `smtp://…?tls=required` on 587. Without it, outgoing email waits in `outgoing`. |
| `info.max_email_bytes` | 10 MB | The largest email the door takes (RP-6 §2.1). |
| `info.max_air_bytes` | 64 kB | The largest message for the radio; larger attachments are held (RP-6 §2.4) in `held/`, each named by its `ref` in hex, until the user asks for them by radio (RP-6 §2.5). |

`/etc/systemd/system/radiopost-po.service`:

```ini
[Unit]
Description=RadioPost post office
After=network-online.target stalwart.service

[Service]
ExecStart=/usr/local/bin/radiopost-po --dir /var/lib/radiopost serve
User=radiopost
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

`systemctl daemon-reload && systemctl enable --now radiopost-po`. It logs what it does with email
to the journal (`journalctl -u radiopost-po`): mail sent out (`email out: …`), and each decision
of the door, with the addresses and the reason but never the message:

```
door: took <maria@example.com> to <kk4oda@radiopost.org> at DATA, 250 2.0.0
door: refused <stranger@example.net> to <kk4oda@radiopost.org> at RCPT TO, 550 5.7.1: the envelope sender is not on the accept list
door: held back <maria@example.com> to <kk4oda@radiopost.org> at DATA, 451 4.7.1: the sender could not be checked for now
```

Stalwart reads changes made in its administration only when it reloads its settings: restart it
(`systemctl restart stalwart`) after §5's changes, or the hook is not called at all. To see that
the hook is called: stop `radiopost-po`, send a message to `postmaster@` on port 25 from the
server itself (`swaks --server 127.0.0.1 --to postmaster@radiopost.org --from test@example.org`),
and Stalwart should answer `451 4.3.5` at `RCPT TO`; start it again.

## 7. DNS

When everything above works, replace the null MX, SPF and DMARC records and publish the rest:

```
radiopost.org.                         MX   10 po.radiopost.org.
radiopost.org.                         TXT  "v=spf1 mx -all"
po.radiopost.org.                      TXT  "v=spf1 a -all"
v1-rsa-<date>._domainkey.radiopost.org.      TXT  "v=DKIM1; k=rsa; h=sha256; p=…"   (Stalwart's zone file)
v1-ed25519-<date>._domainkey.radiopost.org.  TXT  "v=DKIM1; k=ed25519; h=sha256; p=…"
_dmarc.radiopost.org.                  TXT  "v=DMARC1; p=quarantine; rua=mailto:postmaster@radiopost.org"
_radiopost.radiopost.org.              TXT  (what init printed)
```

Start DMARC at `p=quarantine`, and move to `p=reject` after a few weeks of clean reports. Leave out
the zone file's TLSA records unless the domain is signed with DNSSEC: without it every receiver
ignores them, and Caddy makes a new key at each renewal, so a stale record would block mail the day
DNSSEC is turned on. CalDAV, CardDAV, JMAP and `ua-auto-config` records are of no use to
RadioPost; MTA-STS needs a certificate for `mta-sts.<domain>` and can come later. The `_imaps` and
`_submissions` SRV records and the `autoconfig`/`autodiscover` names help mail programs find their
settings.

## 8. Users

**Applications.** Turn them on in the desk's Settings (*Take applications*). A licensee then
chooses *Apply for an account* in their client's Setup: their address, their callsign, their name
as on the licence and an email address to be reached at (RP-5 §6.7). You are emailed, and the
application waits in the desk's Applications with what the post office found of the callsign
(below). You decide:

* **Approve**: the account is made and its credential issued; the applicant's client takes it the
  next time it asks (every minute while it is open). Tick *Role mailbox* for a shared mailbox.
* **Decline**, with a sentence the applicant reads.
* **Send a postcard**: a code good for 30 days, and a card to print with the licence's mailing
  address. Whoever holds the card can read the address on the licence: it shows the applicant
  lives there. The FCC's public data has no email addresses, so the post is the address on the
  licence that can be checked.

**What the post office finds.** In Settings, *Licence lookups*: a copy of the FCC's licence
database kept on the server (fetched each week, about 200 MB, 170 MB kept; lookups stay on the
server), and QRZ.com with your own account. Each application shows green (an active licence in the
applicant's name), amber (something to check: another name, a club callsign, an earlier
application) or red (no licence, or one no longer valid). It is evidence, not proof: anyone can
read who holds a callsign. On the server: `licences --on`, `licences --from l_amat.zip`,
`qrz --username U --password P`, `lookup CALL --name NAME`.

**Mailboxes.** The mail server refuses email for an account with no mailbox, so each approved
account needs one in Stalwart. In Settings, *Mailboxes*: give Stalwart's address as the post
office reaches it (`http://127.0.0.1:8080`) and an API key made in Stalwart's administration with
the right to create accounts, then **Check** (it asks Stalwart for the domain, and makes nothing).
Each approval then makes the mailbox, and the welcome email gives the new user its first password
and where mail programs connect. For another mail server, or a Stalwart whose interface differs,
name a program instead: it is given the address and the name, and the password in
`RADIOPOST_PASSWORD`. On the server: `mailboxes --stalwart URL --key KEY --check`. A mailbox that
could not be made is said in the Decided list; make it in Stalwart's administration as before.

**Codes**, as before, for someone you know holds the callsign: the desk's *New code* (with *Check
the licence*), or

```
radiopost-po --dir /var/lib/radiopost code --address facundo@radiopost.org \
    --operator KK4ODA --name Facundo
```

a one-time code, good for a week, for the user's client to enroll with (RP-5 §6.1). `--name` is the
display name on the email the user sends (`name` changes it later). On the server, `applications`,
`approve ID`, `decline ID --text TEXT` and `postcard ID` do what the desk does.

**The emails** (applications waiting, welcomes, declines, postcards, checks failing) go through
the mail server's submission port (`smtp` in `config.json`) from the contact address.

## 9. Checks before going live

`check` looks at the post office from outside, as the world will: the `_radiopost` records
against `key.hex` and the base URL, MX, SPF, DMARC and the DKIM keys (give the selectors from
Stalwart's zone file), the reverse DNS of the mail host's addresses, the signed `info` document
over HTTPS, the door with its token, and the submission port: it signs in as `radiopost` and asks
to send as `postmaster@`, then resets, so no mail is sent.

```
radiopost-po --dir /var/lib/radiopost check --dkim v1-rsa-20261110,v1-ed25519-20261110
```

Each line says `ok`, `FAIL` (with what to change) or `note`; the last says `ready` when nothing
failed. Then:

1. From an outside address, send an email to a user. It is refused at `RCPT TO` with the accept
   list's explanation (RP-6 §2.3).
2. The user adds that address to their accept list (their client, or by writing to it first).
   Send again: it is taken, and `status` shows the mailbox has one more message.
3. The user calls a gateway: the message is in the waiting list, and comes down.
4. The user answers by radio: within a minute the answer arrives at the outside address, with a
   `Radio-Path:` header and the footer, signed by DKIM (the receiver's "show original" says
   `dkim=pass`, `spf=pass`, `dmarc=pass`).
5. Send an email with two photos of about 40 kB each: together they are over 64 kB, so it comes
   down with both listed as held. The user asks for one (`radiopost attachments ID NAME`, or *Ask
   on the next call* in the panel) and calls again: the gateway says *Your attachments are in a
   new message*, and the next call brings it, from `postmaster@`. A file over 64 kB on its own
   cannot come by radio: asking for it gets *too large*.
6. Send an encrypted email (PGP): refused at `DATA`.
7. Write to `postmaster@` from outside: it arrives in Stalwart's mailbox for it.

## 10. Updating

The post office says when a newer release is out: in the desk, in its log when it starts and
every six hours (`journalctl -u radiopost-po`), in `check`, and in `status`. The desk's Server tab
runs the job **update**, which does what follows. On the server:

```
sudo radiopost-po update
sudo systemctl restart radiopost-po
```

`update` fetches the newest release's tools archive for this machine, checks that the project's
update key signed it, and replaces `radiopost-po` (and any other RadioPost program beside it in
`/usr/local/bin`). It changes nothing when the signature does not match. The post office's
directory, its key and its state are not touched.

## 11. What is not built yet

* **Enrollment with a Logbook of the World certificate** (D33, wishlist W10): an application is
  decided by a person, or by the postcard.
* **The two mailboxes are separate.** Mail read in a mail program is still offered by radio
  (`radio_skips_read` is not built), mail sent by radio does not appear in the mail program's
  Sent folder, mail a user sends from a mail program to another user here stays in the mail
  programs, and mail between two users of the same post office sent by radio is delivered by radio
  without passing through Stalwart.
* **DKIM keys are not rotated** while DNS is managed by hand.
* **Copies placed ahead**, the offline queue and reports are Phase 3; forwarding by radio is
  Phase 4.

## 12. The desk

The **Post office** button of the RadioPost client, for the callsigns that operate the post office
(the info document's `operator`, and any more in `config.json`'s `operators`). It speaks RP-5 §7 and
the service's own additions over HTTPS, as your credential.

| Tab | Has |
|---|---|
| Applications | Those waiting, with the lookups' evidence: Approve, Decline, Send a postcard, Look up again. Those decided in the last week, with whether the mailbox was made and the applicant emailed, and each postcard's code with *Print the card*. |
| Accounts | Every account, its name (*Rename*) and its devices, each with *Revoke* for a lost one; the computer you are using is marked. |
| New code | A code for a new account or a new device, after *Check the licence*. |
| Mail out | What waits to go on by email. |
| Health | Every check, as `check` does it from outside, and what only the server sees: free disk, email stuck going out, last night's backup, whether the jobs runner is installed, a newer release. The service checks every hour and emails the contact address when a check turns red, and when all pass again. Give the DKIM selectors here once. |
| Server | **Jobs** the server's administrator does for you: update, restart the post office or the mail server, copy the certificate, the services, both logs, disk, firewall, ports, logins, Debian's updates, `check`. The service asks; a root-owned systemd unit (`radiopost-po-jobs.path`) runs the job from a fixed list, programs and arguments with no shell, and its output comes back here. The installer puts the unit in place (`radiopost-po --dir /var/lib/radiopost jobs --install`). **Backups**: one each night beside the service, the last seven; *Download a backup* seals one with a passphrase of yours (PBKDF2, then ChaCha20-Poly1305) and keeps it on your computer in `RadioPost backups`. **When you need more**: a terminal on the server through your computer's own SSH, and *Copy for Claude*, the server's state (versions, checks, settings, the jobs' last output; no mail, names or keys) to paste into a Claude session. |
| Settings | Take applications; licence lookups (the FCC copy, QRZ); mailboxes; who operates it. |

To restore a backup on a new server: install it (above), stop the service, then

```
radiopost-po --dir /var/lib/radiopost restore radiopost.org-2026-11-12.rpbackup --force --passphrase-file pass.txt
systemctl start radiopost-po
```

The mail server keeps its own mailboxes: back up `/var/lib/stalwart` with Stalwart stopped, or
with its own tools.
