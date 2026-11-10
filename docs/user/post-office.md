# Running a post office

How to run a RadioPost post office for a domain: the RadioPost service (`radiopost-po`) beside the
Stalwart mail server, on one server, for the pilot (`radiopost.org`, design D29). The service
speaks RP-5 to gateways and clients, and RP-6 to the mail server: it decides at the door which
email comes in, makes each email a RadioPost message, and sends the mail users write by radio out
as email.

This guide is written before the pilot's server exists. The service's side is built and tested;
the Stalwart settings below are to be checked against the version installed, and this guide
corrected then.

---

## 1. What runs where

| Program | Ports | Does |
|---|---|---|
| Stalwart | 25 (SMTP in and out), 587 (submission), 993 (IMAP), its web interface behind the front | Receives and sends email, stores the mailboxes users read in ordinary mail programs, signs outgoing mail (DKIM), checks incoming mail (SPF, DKIM, DMARC, spam). |
| `radiopost-po` | 8770 (the API, behind the front), 8771 (the operator's commands, loopback only), 8772 (the door, loopback) | Accounts, credentials, sessions from gateways, the radio mailboxes, the door, email in and out. |
| An HTTPS front (Caddy or nginx) | 443 | Certificates; `https://po.<domain>/rp/v1/` to `127.0.0.1:8770`, and the mail server's web interface. |

Mail between the service and Stalwart stays on the machine: Stalwart calls the door at
`http://127.0.0.1:8772/` during each SMTP conversation, and the service submits outgoing email to
Stalwart on port 587.

## 2. What the server needs

These decide which provider can host it:

* **Port 25 open both ways.** Many providers block outbound port 25 for new accounts, some lift it
  on request, some never. Ask before choosing; without it, outgoing mail needs a relay service.
* **Reverse DNS** that the operator sets: the server's address must name the mail host
  (`po.radiopost.org`), and that name must resolve back to the address (RP-6 §1).
* **A fixed IPv4 address**, and IPv6 if offered, whose reputation is clean (check it against the
  common block lists before going live).
* **A small machine** is enough for a pilot with a handful of users: Stalwart and `radiopost-po`
  are each one program with no database server.
* Linux, with `systemd`. The release's tools archive has `radiopost-po` for Linux (x86-64).

## 3. Make the post office

```
radiopost-po --dir /var/lib/radiopost init --domain radiopost.org \
    --base-url https://po.radiopost.org/rp/v1 --operator KK4ODA \
    --contact postmaster@radiopost.org --smtp 'smtp://radiopost:PASSWORD@127.0.0.1:587'
```

It makes the post office's key (`key.hex`: keep it, back it up, never share it), its settings
(`config.json`) and an empty state, and prints:

* the DNS records to publish: the `_radiopost` TXT records (RP-1 §7.1);
* the line each user's client trusts it with.

`config.json` holds what `init` was given. Its email settings:

| Member | Default | Meaning |
|---|---|---|
| `hooks` | `127.0.0.1:8772` | Where the door listens for Stalwart's MTA hooks. Off the loopback interface it needs `hook_token`. |
| `hook_token` | none | A bearer token Stalwart sends with each hook (`--hook-token`). |
| `smtp` | none | Stalwart's submission port as a URL. `smtp://` is unencrypted, for the loopback interface only; `smtp://…?tls=required` or `smtps://` across a network. Without it, outgoing email waits in `outgoing`. |
| `info.max_email_bytes` | 10 MB | The largest email the door takes (RP-6 §2.1). |
| `info.max_air_bytes` | 64 kB | The largest message for the radio; larger attachments are held (RP-6 §2.4). |

## 4. DNS

For the domain, besides the `_radiopost` records `init` printed (RP-6 §1):

```
radiopost.org.                MX   10 po.radiopost.org.
po.radiopost.org.             A    <the server's address>
radiopost.org.                TXT  "v=spf1 mx -all"
<selector>._domainkey.radiopost.org.  TXT  "v=DKIM1; k=rsa; p=…"   (from Stalwart)
_dmarc.radiopost.org.         TXT  "v=DMARC1; p=quarantine; rua=mailto:postmaster@radiopost.org"
```

and the reverse DNS of the server's address set to `po.radiopost.org` at the provider. Until the
post office is live, the domain keeps its null MX, SPF and DMARC records.

## 5. Stalwart

In Stalwart's administration:

1. **The domain** `radiopost.org`, with a DKIM key (RSA: RP-6 §1); publish the record it gives.
2. **An account for each user**, the same address as their RadioPost account, so that they can
   also read their mail in an ordinary mail program.
3. **An account `radiopost`** for the service, allowed to send as any address of the domain; its
   password goes in `smtp`.
4. **The MTA hook**: URL `http://127.0.0.1:8772/`, stages **RCPT** and **DATA**, a header
   `Authorization: Bearer <hook_token>` if one is set, and *temporary failure on error*, so that a
   stopped service delays mail rather than letting it all through. In Stalwart's configuration
   file this has been written approximately as:

   ```toml
   [session.hook."radiopost"]
   enable = true
   url = "http://127.0.0.1:8772/"
   stages = ["rcpt", "data"]
   timeout = "30s"
   options = { tempfail-on-error = true }
   ```

The door reads both the shape of request Stalwart's MTA hooks have used (`context.stage`,
`message.headers` and `contents`) and the MTA Hooks Internet-Draft's (`stage`, `rawMessage`),
and answers in the shape it was asked in. DMARC and DKIM come from Stalwart's
`Authentication-Results` (or the draft's `senderAuth`).

## 6. Run it

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

It logs what it does with email out (`email out: …`) to the journal.

## 7. Users

```
radiopost-po --dir /var/lib/radiopost code --address facundo@radiopost.org \
    --operator KK4ODA --name Facundo
```

gives a one-time code, good for a week, for the user's client to enroll with (RP-5 §6.1), once the
operator has seen the user's licence (D33). `--name` is the display name on the email the user
sends (`name` changes it later). `status` and `outgoing` show what the post office holds.

## 8. Checks before going live

1. From an outside address, send an email to a user. It is refused at `RCPT TO` with the accept
   list's explanation (RP-6 §2.3).
2. The user adds that address to their accept list (their client, or by writing to it first).
   Send again: it is taken, and `status` shows the mailbox has one more message.
3. The user calls a gateway: the message is in the waiting list, and comes down.
4. The user answers by radio: within a minute the answer arrives at the outside address, with a
   `Radio-Path:` header and the footer.
5. Send an encrypted email (PGP): refused at `DATA`.

## 9. What is not built yet

* **Held attachments** are kept in `held/` in the post office's directory, but a user cannot yet
  ask for them by radio (RP-5 §5.3 `files`).
* **The two mailboxes are separate.** Mail read in a mail program is still offered by radio
  (`radio_skips_read` is not built), mail sent by radio does not appear in the mail program's
  Sent folder, and mail between two users of the same post office is delivered by radio without
  passing through Stalwart.
* **Copies placed ahead**, the offline queue and reports are Phase 3; forwarding by radio is
  Phase 4.
