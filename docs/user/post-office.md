# Running a post office: the server guide

**Setting up and looking after a RadioPost post office on a server of your own.**

| | |
|---|---|
| **Status** | Current |
| **Version** | 1.0.1 |
| **Updated** | 2026-11-17 |
| **Describes** | RadioPost 0.1.0-beta.27 |
| **Licence** | CC BY 4.0 |

This guide shows a club or a group how to run its own RadioPost post office: rent a server, set it
up with one program that does most of the work, test it, and look after it. It follows the steps
that set up the project's own post office, `radiopost.org`. You type commands, but you do not need
to know how mail servers work.

Once it runs, the post office is looked after from the RadioPost client's **post office desk**,
which [the user's manual](../manual/manual.md#part-4-the-post-office-desk) covers tab by tab. This
guide covers the server: setting it up, and what the desk cannot do.

## Contents

<!-- toc -->

**[Part 1. Before you start](#part-1-before-you-start)**

- [1. About this guide](#1-about-this-guide)
- [2. What a post office is](#2-what-a-post-office-is)
- [3. What you need](#3-what-you-need)

**[Part 2. Setting up](#part-2-setting-up)**

- [4. Rent the server and log in](#4-rent-the-server-and-log-in)
- [5. Run the installer](#5-run-the-installer)
- [6. Your own RadioPost and the desk](#6-your-own-radiopost-and-the-desk)
- [7. Test before going live](#7-test-before-going-live)

**[Part 3. Looking after it](#part-3-looking-after-it)**

- [8. Every day: the desk](#8-every-day-the-desk)
- [9. Users](#9-users)
- [10. Updating](#10-updating)
- [11. Backups and restoring](#11-backups-and-restoring)
- [12. Rotating the DKIM keys](#12-rotating-the-dkim-keys)
- [13. Other post offices](#13-other-post-offices)
- [14. Routes for a cut-off area](#14-routes-for-a-cut-off-area)
- [15. One mailbox: radio and mail programs](#15-one-mailbox-radio-and-mail-programs)
- [16. A backup mail server](#16-a-backup-mail-server)

**[Part 4. Reference](#part-4-reference)**

- [17. Programs and ports](#17-programs-and-ports)
- [18. The setup by hand](#18-the-setup-by-hand)
- [19. The post office's settings](#19-the-post-offices-settings)
- [20. Commands](#20-commands)
- [21. Troubleshooting](#21-troubleshooting)
- [22. What is not built yet](#22-what-is-not-built-yet)
- [23. Glossary](#23-glossary)

**[Appendix](#appendix)**

- [Where to read more](#where-to-read-more)
- [Keeping this guide current](#keeping-this-guide-current)
- [Change log](#change-log)

<!-- /toc -->

---

# Part 1. Before you start

## 1. About this guide

### Which version this is

- **Version** is this guide's own version. It goes up each time the guide changes. The
  [change log](#change-log) at the end says what changed.
- **Describes** is the RadioPost release the guide is written for. It moves with every release.

The copy in the repository follows the programs as they are being built. Releases after
0.1.0-beta.21 carry their own copy as a PDF, `RadioPost-Server-Guide-<release>.pdf`, beside the
installers. To make it yourself: `python tools/manual/build.py pdf server` (it needs Chrome or
Chromium), or `html server` for a page any browser prints.

### How the guide is written

- Grey boxes hold commands. Type them on the server, as **root**, unless the step says otherwise.
  Copy them whole: most are one line.
- The examples use the project's names. Put your own in their place:

  | In the examples | Put your own |
  |---|---|
  | `radiopost.org` | Your post office's domain. |
  | `po.radiopost.org` | `po.` and your domain: the server's name. |
  | `KK4ODA` | Your callsign: the post office's operator. |
  | `postmaster@radiopost.org` | Where people write to you about the post office. |

- **Bold** is something on a screen you click or read. *Italics* is what a program says.
- A box like this one is a note you should not skip:

> **Note.** Some steps you do by hand, at your DNS host, at your server's provider, or on the mail
> server's setup page. The installer stops at each one, says exactly what to type, and waits.

## 2. What a post office is

A post office keeps its users' mailboxes and exchanges their mail with the rest of the world. It is
three programs on one server:

![The server holds Caddy, radiopost-po, Stalwart and the jobs and backups; RadioPost programs reach it over HTTPS, mail programs over 465 and 993, the world's email over 25](fig/overview.svg)

*Figure 1. What runs on a post office's server.*

- **Caddy**, the web server. It gets the server's certificate and keeps it renewed, and passes
  the post office's traffic on.
- **Stalwart**, the mail server. It sends and receives ordinary email, and keeps a mailbox for
  each user that they can also read in a mail program such as Outlook or Thunderbird.
- **radiopost-po**, the post office. It keeps the accounts, issues the credentials, takes the
  sessions gateways pass on, decides which email may come in (*the door*), and sends out as email
  what users write by radio.

Email comes in and goes out like this:

![Coming in: an email reaches Stalwart, the door decides, the post office keeps it for the radio and the mailbox. Going out: a user's session reaches the post office, which makes the email; Stalwart signs and sends it](fig/mail.svg)

*Figure 2. How email comes in and goes out.*

There is no administration page on the Internet and no administrator password. You run the post
office from the RadioPost client on your own computer: your credential is your login.

## 3. What you need

**A domain.** One you control at a DNS host where you can add records, such as `radiopost.org`.
Users' addresses will be `name@` that domain.

**A server.** A small virtual server is enough for a club. The pilot runs on a Hostinger KVM 1 (one
core, 4 GB) with Debian 13. Before you rent one, check these with the provider:

- [ ] **Port 25 open both ways.** Many providers block outgoing port 25 for new accounts. Some lift
  it on request, some never. Ask before you pay. Hostinger does not block it, but limits each
  server to 5 emails a minute: the setup keeps under that.
- [ ] **Reverse DNS you can set.** Each of the server's addresses must name the mail host
  (`po.radiopost.org`). Gmail is strict about it, over IPv6 too.
- [ ] **A fixed IPv4 address with a clean name.** Look it up on Spamhaus and MXToolbox as soon as
  you have it. A provider cannot get an address off someone else's block list.
- [ ] **Debian 13** (or another Linux with systemd), on an x86-64 machine.

**Your own computer**, with RadioPost installed ([the user's manual](../manual/manual.md#4-installing-radiopost))
and an SSH client: Windows, macOS and Linux all have one.

**Your callsign.** The post office's settings name it as the operator: whoever enrolls with that
callsign gets the desk.

**About two hours**, plus waiting for DNS changes to reach the world (minutes to an hour).

---

# Part 2. Setting up

## 4. Rent the server and log in

1. **Make an SSH key on your own computer**, once. In a terminal (on Windows, PowerShell):

   ```
   ssh-keygen -t ed25519
   ```

   Press **Enter** to keep the usual file. A passphrase is a good idea. The public half is the file
   ending in `.pub`, such as `C:\Users\you\.ssh\id_ed25519.pub`.
2. **Rent the server** with Debian 13. When the provider asks for an SSH key, paste the line in
   the `.pub` file. Note the server's IPv4 address.
3. **Log in** from your computer, with the address the provider gave:

   ```
   ssh root@203.0.113.10
   ```

4. **Check that outgoing port 25 is open.** On the server:

   ```
   timeout 10 bash -c '</dev/tcp/gmail-smtp-in.l.google.com/25' && echo open
   ```

   It should say `open`. If not, ask the provider to open it before you go on.
5. **Check the address's reputation** on Spamhaus and MXToolbox's blacklist check. If it is
   listed, ask the provider for another address now.

## 5. Run the installer

The installer, `radiopost-po install`, does the setup in eight steps. Each step says what it does
and why, checks whether it is done already, and does only what is left. Three steps need you for a
moment; it says so and waits.

![The eight steps: updates and firewall, the server's name, SSH by key, Caddy, Stalwart, the post office, DNS, the check; steps 2, 5 and 7 marked as partly by hand](fig/roadmap.svg)

*Figure 3. Setting up a post office.*

1. **Download the RadioPost tools** onto the server:

   ```
   curl -LO https://github.com/KK4ODA/RadioPost/releases/download/v0.1.0-beta.27/radiopost-tools-linux-x64.zip
   apt-get install -y unzip
   unzip radiopost-tools-linux-x64.zip
   ```

2. **See what it would do**, without doing anything:

   ```
   ./radiopost-tools-linux-x64/radiopost-po install --domain radiopost.org --operator KK4ODA --contact postmaster@radiopost.org --dry-run
   ```

3. **Run it** for real: the same line without `--dry-run`.

   ```
   ./radiopost-tools-linux-x64/radiopost-po install --domain radiopost.org --operator KK4ODA --contact postmaster@radiopost.org
   ```

Each step starts with its number and name, such as `[4/8] The web server and its certificate
(Caddy)`, then the line saying why, then each command it runs (after `$`). A step already done says
*already done*. When a step needs you, it prints **By hand:** with what to do, then asks:

```
    Done? Type y and Enter to go on (anything else stops here):
```

Do what it says, then type `y` and **Enter**. Anything else stops the installer; run it again later
and it carries on where it stopped.

> **Note.** The installer makes two secrets: the door's token and the password the post office uses
> at the mail server. It keeps them in `/root/radiopost-install.json`, and shows them only where you
> must type them. Do not share that file.

If a command fails, the installer stops and names it. Put it right and run the installer again:
what is done stays done.

The sections below are the three steps you help with, and the last one.

### 5.1 Step 2: the server's name

The installer names the server `po.radiopost.org`. Mail servers check that a server's name and
address agree, so:

1. **At your DNS host**, add an **A** record: name `po`, value the server's IPv4 address
   (`hostname -I` on the server shows it first). If the server has an IPv6 address, add an
   **AAAA** record too.
2. **At your server's provider**, set the **reverse DNS** (PTR) of each address to
   `po.radiopost.org`. At Hostinger: hPanel, **VPS**, **Settings**, **IP address**, **Set PTR
   record**.

Caddy needs the A record to get its certificate in step 4. Type `y` and **Enter** once both are in.

### 5.2 Step 5: the mail server's setup page

The installer installs Stalwart. Its first setup is a page in a web browser, which the firewall
keeps closed to the Internet. You reach it through your SSH connection:

1. **On your own computer**, in a second terminal, open a tunnel:

   ```
   ssh -L 8080:127.0.0.1:8080 root@po.radiopost.org
   ```

2. **Open** `http://localhost:8080/admin` in your browser. Sign in as `admin` with the temporary
   password Stalwart printed. To see it again, on the server:

   ```
   journalctl -u stalwart -n 200 | grep -A8 'bootstrap mode'
   ```

3. **The setup wizard**, screen by screen:

   | Screen | Set |
   |---|---|
   | 1 | **Server Hostname** `po.radiopost.org`. **Default Email Domain** `radiopost.org`. **Untick** *Automatically Obtain TLS Certificate*: Caddy has the certificate. Keep *Generate Email Signing Keys*. |
   | 2, Storage | As offered (RocksDB in `/var/lib/stalwart/`). |
   | 3, Account directory | The internal directory. |
   | 4, Logging | As offered (a log file in `/var/log/stalwart/`). |
   | 5, DNS | **Manual** DNS server management. |

   Keep the `admin@radiopost.org` password the last screen shows: it is Stalwart's administrator
   from now on.

4. **In Stalwart's administration**, make nine settings. The search box at the top finds each page
   by its name. The installer printed these with your own values; the table says why each matters.

   | # | Page | Set | Why |
   |---|---|---|---|
   | 1 | **Certificates** | A new one, both parts of type *File*: certificate `/etc/stalwart/tls/po.radiopost.org.crt`, private key `/etc/stalwart/tls/po.radiopost.org.key`. | The mail ports use Caddy's certificate, which a daily job copies there. |
   | 2 | **Listeners** | Delete `https`. Set `http` to bind `127.0.0.1:8080`. | Caddy holds 443; only Caddy reaches Stalwart's web page. |
   | 3 | **HTTP, General** | Client IP source *X-Forwarded-For*, 1 hop. | Without it every visitor seems to come from Caddy, and one person guessing passwords gets everyone banned. |
   | 4 | **Sender Authentication** | DMARC verify *strict* on port 25. | Mail that fails DMARC for a domain that says `p=reject` is refused. |
   | 5 | **Rates & Quotas, Outbound Rate Limits** | Only if your provider limits mail (Hostinger does): a new limit, no key, rate 4 per 60000 ms. | Mail over the limit waits in the queue instead of being refused. |
   | 6 | **Directory, Accounts** | An account `radiopost`, with the password the installer printed. | The post office sends users' mail out through it. |
   | 7 | **AUTH Stage, Must Match Sender** | `false` if `authenticated_as == 'radiopost' \|\| authenticated_as == 'radiopost@radiopost.org'`, otherwise `true`. | Lets the post office send as each user's address. Name both forms: Stalwart may give either. |
   | 8 | **MTA Hooks** | URL `http://127.0.0.1:8772/`; stages *rcpt* and *data*; Bearer, with the token the installer printed; timeout 30000 ms; keep *temporary failure on error*; enabled for every session (`true`). | This is the door: Stalwart asks the post office before it takes each email. |
   | 9 | **Directory, API keys** | A key with the right to create accounts. Keep it: you give it to the desk in [chapter 6](#6-your-own-radiopost-and-the-desk). | Approving an application makes the user's mailbox; the post office fills Sent folders and marks mail read. |

   > **Note.** The API key of setting 9 needs these rights. A key in *Inherit* mode on an
   > administrator's account has them all. A key in *Replace* mode needs `authenticate`,
   > `impersonate`, `sysAccountGet`, `sysAccountQuery`, `jmapEmailQuery` and `jmapEmailGet` (to make
   > mailboxes and see what users read), `jmapEmailUpdate`, `jmapEmailImport`, `jmapMailboxQuery`,
   > `jmapMailboxGet` and `jmapBlobUpload` (for Sent folders and read marks), and `sysDomainGet`,
   > `sysDkimSignatureGet`, `sysDkimSignatureQuery`, `sysDkimSignatureCreate`,
   > `sysDkimSignatureUpdate` and `sysDkimSignatureDestroy` (for rotating the DKIM keys). The post
   > office never keeps a user's password.

5. **Restart Stalwart**, on the server, so it reads the new settings:

   ```
   systemctl restart stalwart
   ```

6. Close the tunnel. Back in the installer, type `y` and **Enter**.

> **Note.** From now on, Stalwart's administration is at `https://po.radiopost.org/admin`, signed
> in as `admin@radiopost.org`. You need it rarely: the desk does the daily work.

### 5.3 Step 7: the DNS records

The world finds your post office, and trusts its mail, by its DNS records. The installer prints the
RadioPost records for your domain; publish them with the others below.

![A table of the records: A and AAAA, MX, two SPF records, two DKIM records, DMARC, the _radiopost record and the PTR, each with who reads it](fig/dns.svg)

*Figure 4. The DNS records and who reads them.*

1. **At your DNS host**, publish:

   ```
   po.radiopost.org.             A     (the server's address: already done in step 2)
   radiopost.org.                MX    10 po.radiopost.org.
   radiopost.org.                TXT   "v=spf1 mx -all"
   po.radiopost.org.             TXT   "v=spf1 a -all"
   _dmarc.radiopost.org.         TXT   "v=DMARC1; p=quarantine; rua=mailto:postmaster@radiopost.org"
   _radiopost.radiopost.org.     TXT   (what the installer printed)
   ```

   If the domain had placeholder records saying it sends no mail (a null MX, `v=spf1 -all`,
   DMARC `p=reject`), replace them with these.
2. **The two DKIM records.** In Stalwart's administration: **Management**, **Domains**, the menu
   beside your domain, **View Zone File**. Copy the two `_domainkey` TXT records (`v1-rsa-…` and
   `v1-ed25519-…`) to your DNS host. Note their names, the *selectors*: you give them to the desk
   later.
3. Leave out the zone file's **TLSA** records unless your domain is signed with DNSSEC, and its
   CalDAV, CardDAV, JMAP and `ua-auto-config` records: RadioPost does not use them. The `_imaps` and
   `_submissions` SRV records and the `autoconfig` and `autodiscover` names help users' mail
   programs find their settings: add them if you like.
4. Type `y` and **Enter**.

> **Note.** Start DMARC at `p=quarantine`. After a few weeks of clean reports to
> `postmaster@`, change it to `p=reject`.

### 5.4 Step 8: the check

The installer ends with the check: the post office seen from outside, as the world sees it. It
looks at the `_radiopost` records, MX, SPF, DMARC and the DKIM keys, the reverse DNS, the post
office's signed information over HTTPS, the door, and the mail server's submission port (it signs
in and asks to send, then stops: no mail is sent).

Each line says `ok`, `FAIL` with what to change, or `note`. The last line says `ready` when nothing
failed. DNS changes take time to reach the world: if a record you just published fails, wait a few
minutes and run the check again:

```
radiopost-po --dir /var/lib/radiopost check --dkim v1-rsa-20261110,v1-ed25519-20261110
```

Use your own DKIM selectors. They are kept, so later checks use them.

When the installer finishes it says *The post office is set up*, and prints the line that makes
your own enrollment code. Keep it for the next chapter.

## 6. Your own RadioPost and the desk

You run the post office from the RadioPost client on your own computer, enrolled as the
operator's callsign.

1. **Make your code**, on the server: the line the installer printed, with your address.

   ```
   radiopost-po --dir /var/lib/radiopost code --address facundo@radiopost.org --operator KK4ODA --name Facundo
   ```

   It prints a one-time code such as `7Q4F-AZ9K`, good for a week.
2. **Enroll**, on your computer: open RadioPost, **Setup**, **I have a code**, fill in your
   address, callsign and the code, and click **Enroll**.

   ![RadioPost's enrollment box, filled in with an address, a callsign and a code](../manual/screens/client-enroll.png)

   *Figure 5. Enrolling as the operator.*

3. A **Post office** button appears in RadioPost's left-hand column: the desk. Open it.
4. **Health**: open **DKIM selectors**, type your two selectors with a comma between them, and
   click **Save**. Every check should now pass.

   ![The desk's Health tab, every check listed](../manual/screens/desk-health.png)

   *Figure 6. The desk's Health tab.*

5. **Settings, Mailboxes**: choose **In Stalwart, by its API**. **Stalwart's address** is
   `http://127.0.0.1:8080`; **API key** is the key you made in step 9 of section 5.2. Click
   **Save**, then **Check**: it asks Stalwart without making anything.

   ![The desk's Settings tab: applications, licence lookups, mailboxes](../manual/screens/desk-settings.png)

   *Figure 7. The desk's Settings tab.*

6. **Settings, Licence lookups**: tick **Look US callsigns up in a copy of the FCC's licence
   database**, and give your QRZ account if you have one.
7. **Server, Backups**: under **On this computer, by itself**, name this computer and click
   **Turn on** ([chapter 11](#11-backups-and-restoring)).
8. **Settings, Applications**: tick **Take applications for accounts** when you are ready for
   people to apply from their RadioPost. Until then, only codes you make let anyone in.

## 7. Test before going live

Make a test account for a second callsign of yours, or a friend's, and go through these with a
RadioPost station and an ordinary email account elsewhere.

1. **An email from outside is refused** until its sender is on the user's accept list. From an
   outside address, write to the user. It is refused, saying the sender is not on the accept list.
2. **Accepted once listed.** The user adds that address to their accept list (in RadioPost's Setup,
   or by writing to it first). Send again: it is taken, and the desk's **Accounts** shows one more
   message waiting.
3. **It comes down by radio.** The user calls a gateway: the message is in the list, and comes
   down.
4. **An answer by radio goes out.** The user answers. Within a minute the answer reaches the
   outside address, with a `Radio-Path:` line in its header. The receiver's "show original" says
   `dkim=pass`, `spf=pass` and `dmarc=pass`.
5. **Large attachments wait.** Send an email with two photos of about 40 kB each: together over
   64 kB, so it comes down with both listed as held. The user clicks **Ask on the next call** on
   one and calls twice: the second call brings it, from `postmaster@`.
6. **Encrypted mail is refused.** Send a PGP-encrypted email: it is refused.
7. **`postmaster@` works.** Write to it from outside: it arrives in Stalwart's mailbox for it.

When all seven pass, the post office is live. Tell your users where to apply, or give them codes.

---

# Part 3. Looking after it

## 8. Every day: the desk

The desk in your RadioPost does the daily work. [The user's manual](../manual/manual.md#part-4-the-post-office-desk)
covers it tab by tab. What to watch:

- **Your email.** The post office checks itself every hour and emails `postmaster@` when a check
  turns red, and again when all pass. It also emails you when someone applies.
- **Applications**: decide each one ([manual, chapter 26](../manual/manual.md#26-applications)).
- **Health**: anything amber or red ([manual, chapter 31](../manual/manual.md#31-health)).
- **Backups**: that a copy left the server in the last week
  ([chapter 11](#11-backups-and-restoring)).

## 9. Users

**Applications.** With applications on, a licensee applies from their RadioPost: the address they
want, their callsign, the name on their licence and an email address. Each waits on the desk with
what the post office found of the callsign. **Approve** makes the account and its mailbox, and
emails the new user their first password and where mail programs connect. **Decline…** sends a
reason. **Send a postcard** makes a code for 30 days and a card to post to the address on the
licence: the FCC's public data has no email addresses, so the post is the address that can be
checked.

**Codes.** For someone you know holds the callsign: the desk's **New code**, or on the server:

```
radiopost-po --dir /var/lib/radiopost code --address ana@radiopost.org --operator N0CALL --name "Ana Lopez"
```

**Mailboxes.** Each account needs a mailbox at Stalwart, or Stalwart refuses mail for it. With
**Mailboxes** set to Stalwart (chapter 6), approving makes it. Otherwise make it in Stalwart's
administration (**Directory**, **Accounts**) with the same address.

**Who runs the post office.** The desk goes by callsign: whoever holds a credential naming one of
the post office's operator callsigns can use it. Nobody becomes an operator by applying: an
application naming an operator's callsign is refused. A code naming an operator's callsign for
someone else's address shows a red warning first.

**From the server**, the same: `applications --open` or `--close`, `applications`, `approve ID`,
`decline ID --text TEXT`, `postcard ID`, `licences --on`, `qrz --username U --password P`,
`lookup CALL --name NAME`, `mailboxes --stalwart URL --key KEY --check`
([chapter 20](#20-commands)).

## 10. Updating

The post office says when a newer release is out: on the desk, in its log, and in the check.

**From the desk:** **Server**, the job **update**, **Run**, and again to confirm. It installs the new
release, its signature checked, and restarts the post office.

![The desk's Server tab: the jobs, with update first](../manual/screens/desk-server.png)

*Figure 8. Jobs on the desk's Server tab.*

**From the server:**

```
sudo radiopost-po update
sudo systemctl restart radiopost-po
```

`update` fetches the newest release for this machine, checks that the project's key signed it, and
replaces the program. It changes nothing if the signature does not match. The post office's folder,
its key and its state are not touched.

**From releases before 0.1.0-beta.14:** in Stalwart's **MTA Hooks**, set the hook's enabled
condition to `true` for every session (section 5.2, setting 8), save, and restart Stalwart. Until
then, mail users send each other from mail programs does not reach the radio.

**From releases before 0.1.0-beta.17:** Sent folders, read marks and the DKIM rotation use the
mailboxes key (chapter 6). A key in *Replace* mode needs the rights in section 5.2's note.

## 11. Backups and restoring

![Each night the server makes two backups; your computer fetches each one by itself, or you download one by hand; either restores on a new server](fig/backups.svg)

*Figure 9. Backups: made on the server, kept off it.*

**What the server keeps.** Each night it makes a backup of the post office (its key, accounts and
state; the last seven kept) and of the mail server's mailboxes, with Stalwart stopped for a moment
(the last three kept). Senders retry for days, so that moment loses no mail. A backup that stays on
the server is lost with it.

**On your computer, by itself.** On the desk: **Server**, **Backups**, **On this computer, by
itself**, a name and a folder, **Turn on**. While RadioPost is open, it fetches each new backup,
sealed to a key only that computer holds.

![The Backups and DKIM keys boxes on the desk's Server tab](../manual/screens/desk-backups.png)

*Figure 10. Backups on the desk.*

> **Note.** The file `backup.key`, in RadioPost's folder on that computer, is what opens those
> backups. Lose the computer and its key, and its backups go with them. Keep a copy of the key on a
> USB stick in a drawer, or turn this on on a second computer of yours too (up to eight can be).

**By hand.** On the desk: a passphrase of 12 characters or more, twice, then **Download a backup**.
It is sealed with that passphrase: without it nobody can open it, you included. From the server:
`radiopost-po --dir /var/lib/radiopost backup --to FILE --passphrase-file F`.

**Restoring on a new server:**

1. Set up the new server with the installer (chapter 5), Stalwart's wizard included.
2. A backup your computer fetched opens there first. On that computer:

   ```
   radiopost backups open radiopost.org-2026-11-12.rpbackup
   ```

   It writes `radiopost.org-2026-11-12.zip` beside it. Copy that to the server. (A backup you
   downloaded with a passphrase goes to the server as it is.)
3. On the server, stop the post office and restore:

   ```
   systemctl stop radiopost-po
   radiopost-po --dir /var/lib/radiopost restore radiopost.org-2026-11-12.zip --force
   systemctl start radiopost-po
   ```

   For a passphrase backup, add `--passphrase-file pass.txt`, a file holding the passphrase.
4. Sign in to a user's mailbox and check that the mail, folders and attachments are there.
5. Delete the unsealed `.zip` from both computers: it is the post office, keys and passwords.

`restore` puts the post office's files back, then the mailboxes: it stops Stalwart, moves its data
aside as `…before-restore-<time>` (delete those once all is well), unpacks the backup's, and starts
Stalwart again. `--without-mail` leaves the mail server alone.

## 12. Rotating the DKIM keys

The mail server signs outgoing mail with its DKIM keys. Health turns amber when they are older than
180 days. Since you edit DNS yourself, the desk rotates them with you, one checked step at a time:
start, publish the new records, switch, wait a week, remove the old records, end. The
[user's manual, chapter 33](../manual/manual.md#dkim-keys) has the steps.

The desk needs **Mailboxes** set to Stalwart with its API key (chapter 6). The post office never
keeps a private key: it makes each one in memory and hands it to Stalwart.

## 13. Other post offices

Your users' mail reaches every post office as ordinary email. A post office you **know** is one
whose word you take that its users are licensed: its users' mail needs no place on accept lists and
goes on the air without the third-party flag, its gateways log in here, and your users' clients
list its gateways. Know only post offices whose operators you trust to check callsigns.

The desk's **Post offices** tab knows, forgets and blocks them
([manual, chapter 30](../manual/manual.md#30-other-post-offices)). From the server:

```
radiopost-po --dir /var/lib/radiopost know --domain hampost.org
radiopost-po --dir /var/lib/radiopost post-offices
radiopost-po --dir /var/lib/radiopost post-offices --block spam-relay.example
```

`know` shows what it found in DNS before it takes the post office. The service fetches each known
post office's records, information and list of revoked credentials at once and every six hours. If
a fetch fails it keeps what it had; after a day, Health turns red.

## 14. Routes for a cut-off area

A **route** says which gateway holds the mail of another gateway's users while that gateway is cut
off from the Internet. Set one for each gateway serving an area likely to lose the Internet,
naming a gateway outside it that it can reach by radio, whose operator agreed:

```
radiopost-po --dir /var/lib/radiopost route --via N1CALL-10 --users-at N2CALL-5,KK4ODA-1
radiopost-po --dir /var/lib/radiopost routes
radiopost-po --dir /var/lib/radiopost route --via N1CALL-10 --users-at ""
```

The first sets the route through N1CALL-10 (a second `route` with the same `--via` replaces it). The
second lists the routes. The last removes it.

What happens then:

1. A gateway in `--users-at` that has been asking for copies and stops for 15 minutes counts as
   **cut off**.
2. The copies it never confirmed, and every new message for its users, also go to the `--via`
   gateway, signed by the post office.
3. The cut-off gateway collects them when it calls the `--via` gateway by radio, checks the
   signature, and holds them for its users.
4. When its Internet is back, what still waits at the `--via` gateway is dropped.

The `--via` gateway must hold copies or publish windows, so that it asks the post office for them.
Both gateways' operators name each other as partners in RadioPost Gateway
([the user's manual, chapter 22](../manual/manual.md#22-partners-forwarding-by-radio)). The post
office publishes its routes, signed, for anyone to read.

## 15. One mailbox: radio and mail programs

Each user has one mailbox, read by radio and in a mail program:

- Mail a user sends from a mail program to another user here reaches the radio too.
- Mail sent by radio to a user here reaches their mail program too, as an email.
- Mail a user sends by radio goes into their **Sent** folder, and mail they pick up by radio is
  marked read in their mail program.
- A user who reads mail on the Internet can ask not to be offered by radio what they already read
  (*Skip mail I have read in a mail program*, in RadioPost's Setup).

The last two need **Mailboxes** set to Stalwart with its API key (chapter 6). Without it the post
office says so in its log, and the radio works as before.

**What the door checks.** Before Stalwart takes an email for a RadioPost user, the door checks:

- that the sender is on the user's accept list;
- SPF, DKIM and DMARC, itself, never trusting what the message says of itself;
- that the email is not encrypted, and not over the size limit.

A check that cannot finish answers *try again later* (`451`), and the sending server does.
Addresses that are not RadioPost accounts (`postmaster@`, `abuse@`) are Stalwart's alone.

## 16. A backup mail server

A second mail server, listed in DNS after your own, takes your post office's mail while its
server is down, and hands it on when it is back. Without one nothing is lost: senders' servers
retry for days. With one, mail does not wait at senders' servers.

![A sender's mail server finds the post office down and gives the mail to the backup, which judges it with a copy of the accounts and hands it on when the post office is back](fig/backup-mx.svg)

*Figure 11. A backup mail server.*

The backup must refuse what your door refuses, or it would take mail it later has to bounce to
senders who may be forged. So it runs a door of its own, the **backup door**. Every five minutes
it fetches a copy of the accounts and accept lists from the post office, and keeps it for when the
post office cannot be reached. It checks SPF, DKIM and DMARC itself, and seals its verdicts into
each message it passes on, with a key it shares with the post office. Your post office's door takes
those verdicts. A seal anyone else writes does not check, and is ignored.

You need a second server, at another provider if you can.

1. **Make the backup's token**, on the post office's server:

   ```
   radiopost-po --dir /var/lib/radiopost backup-mx --add mx2
   ```

   It prints the token once. Copy it to the backup's server, into a file only root can read, such
   as `/root/mx2.token`. `backup-mx` alone lists the backups; `backup-mx --remove mx2` takes one
   away: its token stops working at once, and its seals are no longer taken.

2. **Set up the backup's server** like the first, with its own name, `mx2.radiopost.org`:
   - its A record and reverse DNS (section 5.1);
   - the firewall and SSH (section 18.1);
   - Caddy and its certificate for `mx2.radiopost.org` (section 18.2). Its only job here is the
     certificate for Stalwart's mail ports: leave the `handle /rp/v1/*` part out of the
     Caddyfile;
   - Stalwart: the wizard and settings 1 to 4 of section 5.2, with `mx2.radiopost.org` as the
     hostname (section 18.3);
   - `radiopost-po` from the tools archive of the same release as the post office, in
     `/usr/local/bin/` (section 18.4, its first four lines).

3. **Make Stalwart on the backup relay**, not keep: it holds no mailboxes for your domain.
   - Allow relaying for recipients at your domain on port 25, with an RCPT rule such as
     `rcpt_domain == 'radiopost.org'`.
   - Route your domain's mail to `po.radiopost.org`, port 25, and keep trying for five days.
   - **MTA Hooks**: URL `http://127.0.0.1:8472/`, stages *rcpt* and *data*, *temporary failure on
     error*. The backup door listens on the server itself only, and needs no token there.
   - **Sender Authentication**: DMARC verify *strict* on port 25, as on the post office.
   - Restart Stalwart.

4. **Start the backup door** the first time. It runs as a `radiopost` user whose folder only it
   can read:

   ```
   useradd --system --home-dir /var/lib/radiopost-backup --create-home --shell /usr/sbin/nologin radiopost
   chmod 700 /var/lib/radiopost-backup
   install -o radiopost -m 0600 /root/mx2.token /var/lib/radiopost-backup/mx2.token
   runuser -u radiopost -- radiopost-po --dir /var/lib/radiopost-backup backup-door --primary https://po.radiopost.org/rp/v1 --token-file /var/lib/radiopost-backup/mx2.token --host mx2.radiopost.org
   ```

   It keeps those settings, the token with them, in its folder, and starts. Its log says when it
   has fetched the accounts; until then it holds every message back with `451`, so senders try
   again. Once it has, stop it with **Ctrl+C**, and delete the token files:

   ```
   rm /root/mx2.token /var/lib/radiopost-backup/mx2.token
   ```

5. **Run it as a service**, `/etc/systemd/system/radiopost-backup-door.service`:

   ```ini
   [Unit]
   Description=RadioPost backup door
   After=network-online.target stalwart.service

   [Service]
   ExecStart=/usr/local/bin/radiopost-po --dir /var/lib/radiopost-backup backup-door
   User=radiopost
   Restart=on-failure

   [Install]
   WantedBy=multi-user.target
   ```

   Then `systemctl daemon-reload && systemctl enable --now radiopost-backup-door`. Its log
   (`journalctl -u radiopost-backup-door`) says when it works from its copy because the post
   office could not be reached.

6. **Let the post office take the backup's mail.** In your post office's Stalwart, **Sender
   Authentication**: make DMARC verify skip the backup's address (`remote_ip ==` the backup's
   address: *disable*; otherwise *strict*). Mail relayed by the backup fails SPF there, and a sender
   who publishes `p=reject` would be refused. The door still judges that mail, with the backup's
   verdicts.

7. **Test it** before the world knows of it. Send a message straight to the backup:

   ```
   swaks --to ana@radiopost.org --server mx2.radiopost.org
   ```

   It should reach the user. A sender not on the user's accept list should be refused at the
   backup.

8. **Publish it in DNS**, last:

   ```
   radiopost.org.   MX   20 mx2.radiopost.org.
   ```

   Your own MX stays at `10`, so senders use the backup only when your post office does not
   answer.

---

# Part 4. Reference

## 17. Programs and ports

| Program | Ports | Does |
|---|---|---|
| Caddy | 80, 443 | Gets and renews the certificate for `po.radiopost.org`; sends `https://po.radiopost.org/rp/v1/…` to the post office and everything else to Stalwart's web page. |
| Stalwart | 25 (email in and out), 465 (mail programs sending), 993 (mail programs reading); its web page on `127.0.0.1:8080`, behind Caddy | Email in and out, the mailboxes, DKIM signing. |
| `radiopost-po` | 8770 (the API, behind Caddy), 8771 (the operator's commands), 8772 (the door): all on `127.0.0.1` | Accounts, credentials, gateways' sessions, the radio mailboxes, the door, email in and out. |

The firewall opens 22, 25, 80, 443, 465 and 993. Stalwart also opens POP3 (995) and ManageSieve
(4190); the firewall keeps them closed. Mail between the post office and Stalwart stays on the
server: Stalwart calls the door at `http://127.0.0.1:8772/`, and the post office sends email out
through Stalwart's port 465 as the account `radiopost`.

Gateways that hold copies of users' mail ask the post office for them and wait up to a minute for
an answer, so each keeps one connection open through Caddy, whose defaults allow it. A backup
mail server's door listens on its own server at `127.0.0.1:8472` (chapter 16).

## 18. The setup by hand

The installer does all of this. This chapter is for setting up without it, on another Linux, or to
see what it does.

### 18.1 The server

```
apt update && apt full-upgrade -y
hostnamectl set-hostname po.radiopost.org
sed -i 's/^127\.0\.1\.1.*/127.0.1.1\tpo.radiopost.org po/' /etc/hosts
```

A provider's image that runs cloud-init would put its own name back at a reboot:

```
printf 'preserve_hostname: true\nmanage_etc_hosts: false\n' > /etc/cloud/cloud.cfg.d/99-radiopost.cfg
```

SSH by key only (the `00-` matters: the first setting read wins):

```
printf 'PasswordAuthentication no\nPermitRootLogin prohibit-password\n' > /etc/ssh/sshd_config.d/00-radiopost.conf
sshd -t && systemctl reload ssh
```

The firewall and nightly security updates:

```
apt install -y ufw unattended-upgrades
ufw default deny incoming
ufw default allow outgoing
for p in 22 25 80 443 465 993; do ufw allow $p/tcp; done
ufw enable
```

Then the server's A record and reverse DNS (section 5.1).

### 18.2 Caddy and the certificate

```
apt install -y curl gnupg
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' > /etc/apt/sources.list.d/caddy-stable.list
chmod o+r /usr/share/keyrings/caddy-stable-archive-keyring.gpg /etc/apt/sources.list.d/caddy-stable.list
apt update && apt install -y caddy
```

`/etc/caddy/Caddyfile` (`issuer acme` keeps the certificate with Let's Encrypt, in a known folder):

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

Then `systemctl reload caddy`. The certificate appears under
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
`stalwart-certs.timer` (`OnCalendar=daily`, `RandomizedDelaySec=1h`, `Persistent=true`) run it each
day. Caddy renews weeks before a certificate ends. Run the service once by hand after Stalwart is
installed.

### 18.3 Stalwart

Install it (Caddy already holds port 443, and Stalwart starts without it):

```
curl --proto '=https' --tlsv1.2 -sSf https://get.stalw.art/install.sh -o install.sh
sh install.sh
```

Then the wizard and the nine settings of section 5.2, using a hook token and a `radiopost` password
of your own making (letters and digits only: `openssl rand -hex 24` makes one). After the wizard:
`systemctl start stalwart-certs`, then:

```
echo 'STALWART_PUBLIC_URL=https://po.radiopost.org' >> /etc/stalwart/stalwart.env
systemctl restart stalwart
grep -h -E 'listen-start|build-error|ERROR' /var/log/stalwart/* | tail -n 20
```

The listeners should be 25, 465, 993, 995, 4190 and `127.0.0.1:8080`, nothing on 443, and no
errors.

### 18.4 The post office

```
apt install -y unzip
curl -LO https://github.com/KK4ODA/RadioPost/releases/download/v0.1.0-beta.27/radiopost-tools-linux-x64.zip
unzip radiopost-tools-linux-x64.zip
install -m 0755 radiopost-tools-linux-x64/radiopost-po /usr/local/bin/
useradd --system --home-dir /var/lib/radiopost --create-home --shell /usr/sbin/nologin radiopost
runuser -u radiopost -- radiopost-po --dir /var/lib/radiopost init --domain radiopost.org \
    --base-url https://po.radiopost.org/rp/v1 --operator KK4ODA \
    --contact postmaster@radiopost.org --hook-token HOOK_TOKEN \
    --smtp 'smtps://radiopost:PASSWORD@po.radiopost.org:465'
```

`init` makes the post office's key (`key.hex`: back it up off the server, never share it), its
settings (`config.json`) and an empty state, and prints the `_radiopost` DNS records.

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

Then:

```
systemctl daemon-reload && systemctl enable --now radiopost-po
sudo radiopost-po --dir /var/lib/radiopost jobs --install
```

The second line installs the runner for the desk's jobs. Then DNS (section 5.3) and the check
(section 5.4).

**The door's log.** The post office logs each decision of the door to the journal
(`journalctl -u radiopost-po`), with the addresses and the reason, never the message:

```
door: took <maria@example.com> to <ana@radiopost.org> at DATA, 250 2.0.0
door: refused <stranger@example.net> to <ana@radiopost.org> at RCPT TO, 550 5.7.1: the envelope sender is not on the accept list
door: held back <maria@example.com> to <ana@radiopost.org> at DATA, 451 4.7.1: the sender could not be checked for now
```

**Is the door called?** Stop `radiopost-po`, send a message to `postmaster@` from the server
(`swaks --server 127.0.0.1 --to postmaster@radiopost.org --from test@example.org`): Stalwart
should answer `451 4.3.5` at `RCPT TO`. Start the post office again.

## 19. The post office's settings

`config.json`, in `/var/lib/radiopost`, holds what `init` was given and what the desk sets. Its
email settings:

| Setting | Default | Means |
|---|---|---|
| `hooks` | `127.0.0.1:8772` | Where the door listens for Stalwart's MTA hooks. Away from `127.0.0.1` it needs `hook_token`. |
| `hook_token` | none | The token Stalwart sends with each hook (`--hook-token`). |
| `smtp` | none | Stalwart's submission port: `smtps://` on 465, or `smtp://…?tls=required` on 587. Without it, outgoing email waits on the desk's **Mail out**. |
| `info.max_email_bytes` | 10 MB | The largest email the door takes. |
| `info.max_air_bytes` | 64 kB | The largest message for the radio. Larger attachments are held until a user asks for them by radio. |
| `compress` | `true` | Messages for the radio go compressed when that makes them smaller: a page of text takes a quarter to a half less air time. |

It also holds `known`, `blocked` and `resolver` for other post offices (chapter 13), and
`operators` for more operator callsigns than the one `init` named.

## 20. Commands

All are `radiopost-po --dir /var/lib/radiopost COMMAND`, but `install` and `update`.

| Command | Does |
|---|---|
| `install --domain D --operator CALL --contact EMAIL [--dry-run] [--yes]` | The whole setup, on a fresh Debian server, as root (chapter 5). |
| `update` | Install the newest release, its signature checked (as root). |
| `init …` | Make a post office: its key, settings and state (section 18.4). |
| `serve` | Run the post office (the systemd service does). |
| `dns` | The `_radiopost` DNS records to publish. |
| `check [--dkim S1,S2]` | The post office checked from outside (section 5.4). |
| `health` | The hourly check, now. |
| `status` | Accounts, gateways, codes, sessions. |
| `code --address A --operator CALL [--role] [--name NAME]` | A one-time code, good for a week. |
| `name --address A --name NAME` | The name on an account's email. |
| `revoke --device KEY` | Revoke every credential of a lost device. |
| `applications [--open \| --close]`, `approve ID [--role]`, `decline ID --text TEXT`, `postcard ID` | Applications for accounts. |
| `licences [--on \| --off \| --fetch \| --from ZIP]`, `qrz --username U --password P`, `lookup CALL [--name NAME]` | Licence lookups. |
| `mailboxes [--stalwart URL --key KEY \| --command PROGRAM \| --off] [--check]` | How approved accounts' mailboxes are made. |
| `outgoing` | Mail waiting to go out by email. |
| `know --domain D`, `post-offices [--block D \| --unblock D \| --forget D \| --refresh]` | Other post offices (chapter 13). |
| `route --via CALL --users-at CALL,…`, `routes` | Routes for a cut-off area (chapter 14). |
| `jobs [--install]` | The runner for the desk's jobs; `--install` puts it in place once (with sudo). |
| `backup --to FILE [--passphrase-file F]`, `restore FILE [--force] [--passphrase-file F] [--without-mail]`, `mail-backup` | Backups (chapter 11). |
| `backup-mx [--add NAME \| --remove NAME]` | The backup mail servers: make one's token, list them, or take one away (chapter 16). |
| `backup-door --primary URL --token-file FILE [--host NAME] [--listen ADDR]` | On the backup's server: its door, for its Stalwart's MTA hook (127.0.0.1:8472). Run again with no flags (chapter 16). |

## 21. Troubleshooting

| You see | Do |
|---|---|
| The port 25 check does not say `open` | The provider blocks outgoing mail. Ask them to open it, or choose another provider. |
| The installer stops at a command | Put right what it names, then run the installer again: what is done stays done. |
| Caddy gets no certificate | `po.radiopost.org` does not point at the server yet. Check the A record (section 5.1), wait for DNS, then `systemctl reload caddy`. |
| The check says a record `FAIL` | Publish it as the line says. DNS takes minutes to an hour to reach the world: check again later. |
| Users' mail out is refused: *501 5.5.4 You are not allowed to send from this address* | Stalwart's **Must Match Sender** rule names only one form of the `radiopost` account. Name both (section 5.2, setting 7) and restart Stalwart. |
| Stalwart refuses the post office's login as `radiopost` | Give the full address in `config.json`'s `smtp`, written `radiopost%40radiopost.org`. |
| Every email for users is let in without checks, or none arrives | The door is not called. Restart Stalwart after its settings (section 5.2), then test the door (section 18.4). |
| Email for a user is refused right after `RCPT TO` | The user has no mailbox at Stalwart. Set **Mailboxes** (chapter 6) or make it by hand. |
| The desk's jobs stay *asked for, waiting for the runner* | `sudo radiopost-po --dir /var/lib/radiopost jobs --install`, once. |
| Health is red | Do what the line says. You are emailed again when all pass. |
| The post office's log says the mail server cannot be read | **Mailboxes** is not set to Stalwart with its API key (chapter 6). |
| Mail through the backup is refused at the post office for DMARC | The post office's Stalwart does not skip DMARC for the backup's address (chapter 16, step 6). |
| The backup holds every message back with `451` | Its door has no copy of the accounts yet: check its log, and that it reaches `https://po.radiopost.org`. |
| The FCC copy never comes: *the fetch failed … http status: 403* | Update to 0.1.0-beta.24 or later (chapter 10): the FCC's site refused the requests of earlier releases. If it still fails, fetch the file on the server with `curl -o /tmp/l_amat.zip https://data.fcc.gov/download/pub/uls/complete/l_amat.zip` and build the copy from it: `sudo -u radiopost radiopost-po --dir /var/lib/radiopost licences --from /tmp/l_amat.zip`. |

## 22. What is not built yet

- **Enrollment with a Logbook of the World certificate.** An application is decided by a person,
  or by the postcard.
- **A user's own mail program counts as third-party mail**, as mail from the Internet does: it is
  held back from stations abroad without an arrangement.

## 23. Glossary

| Word | Means |
|---|---|
| Backup mail server | A second mail server, MX 20, that takes the post office's mail while it is down (chapter 16). |
| Caddy | The web server: the certificate, and HTTPS in front of the post office and Stalwart. |
| Credential | A signed file that proves an address and callsign belong together. The post office issues them. |
| Desk | The post office desk, in the RadioPost client: how operators run the post office. |
| DKIM | A signature on outgoing email, with a key whose public half is in DNS, so receivers can trust it. |
| DMARC | A DNS record saying what receivers do with mail that fails SPF and DKIM. |
| The door | The post office's check on each email before Stalwart takes it, called by Stalwart's MTA hook. |
| Jobs runner | The server's own service that runs the desk's jobs, from a fixed list. |
| MX | The DNS record that says which server takes the domain's email. |
| Operator | A callsign the post office's settings name: its holder gets the desk. |
| PTR | Reverse DNS: the name an address gives back. |
| SPF | A DNS record listing the servers allowed to send the domain's email. |
| Stalwart | The mail server. |
| Tunnel (SSH) | A way to reach a page on the server through your SSH connection, without opening a port. |

---

# Appendix

## Where to read more

| Subject | Document |
|---|---|
| The desk, tab by tab | [The user's manual, Part 4](../manual/manual.md#part-4-the-post-office-desk) |
| Running a gateway | [The user's manual, Part 3](../manual/manual.md#part-3-radiopost-gateway) and [the gateway guide](gateway.md) |
| How RadioPost works | [The concept](../concept/00-concept.md) |
| The post office's protocols | [RP-5, the post office API, and RP-6, email](../../spec/README.md) |
| Stalwart | [stalw.art/docs](https://stalw.art/docs) |
| Releases | [github.com/KK4ODA/RadioPost/releases](https://github.com/KK4ODA/RadioPost/releases) |
| Bugs and ideas | [github.com/KK4ODA/RadioPost/issues](https://github.com/KK4ODA/RadioPost/issues) |

## Keeping this guide current

This guide is part of the programs, as the user's manual is. A change to what an operator sets up,
types or sees on the server (an installer step, a command, a setting, a file, a message) updates it
in the same commit, with a new **Version** and a row in the change log below. `check_docs.py` holds
the header to the change log; **Describes** moves with `tools/release.py bump`; and
`python tools/manual/build.py check` checks its contents, figures and links. Its schematics are
drawn by `tools/figures/server.py`; its screenshots of the desk are the user's manual's.

## Change log

| Version | Date | Changes |
|---|---|---|
| 1.0.0 | 2026-11-15 | Rewritten in the manual's style: step by step around the installer, with schematics and screenshots, a contents, troubleshooting and a glossary. The previous guide's content is kept: the setup by hand is now chapter 18, and the backup mail server chapter 16. |
| 1.0.1 | 2026-11-17 | Troubleshooting: the FCC copy that never comes (the FCC's site refused releases before 0.1.0-beta.24), and building it from a file fetched by hand. |
