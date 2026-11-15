# The public test service

Check your own RadioPost program against RadioPost's, over the Internet. The test service runs a
post office, a gateway (**RPTEST-1**) and a client's station (**RPTEST-2**). Your program enrolls
at the post office, calls one of the stations, and the service's page shows every transmission of
the session, decoded, and the checks your program was held to.

Nothing here goes on the air, and nothing here is real mail. The test post office publishes
nothing in DNS, so no real gateway or post office trusts its credentials (RP-1 §7.3). Part 1 is for
someone writing a program. Part 2 is for whoever hosts the service.

---

## Part 1. Checking your program

### 1.1 Trust the test post office

The service's page gives the test post office's domain, its key and its API's address. Your
program trusts that key by hand, as RP-1 §7.3 allows. With the RadioPost client:

```
radiopost trust test.radiopost.org 4f2a…
```

The page shows the exact line, and `GET /key` gives the same as JSON.

### 1.2 Get an enrollment code

Type your callsign on the page and click **Get a code**. The code is good for a week. The account
is your callsign's base, at the test domain: `kk4oda@test.radiopost.org` for `KK4ODA/P`. Three
messages wait in it for your first call to RPTEST-1: a short note, one with a 6 kB file, and a
priority message.

Enroll as RP-5 §6.1 says, at the API the page names. With the RadioPost client:

```
radiopost enroll --address kk4oda@test.radiopost.org --operator KK4ODA --code 7Q4F-AZ9K \
  --post-office https://test.radiopost.org/rp/v1
```

A script can ask for a code too: `POST /codes` with `{"callsign": "KK4ODA"}` answers JSON with the
code, the account and everything on the page. The service gives out at most 60 codes an hour.

### 1.3 Call

Your program reaches the service through a TCP adapter (RP-4 §9.5). With RadioPost's, name the
two stations as peers and pin the service's certificate, whose fingerprint the page shows:

```
radiopost-tcp --peer RPTEST-1=test.radiopost.org:8774 --peer RPTEST-2=test.radiopost.org:8774 \
  --trust 9c41…
```

Then point your program at that adapter, and call:

* **RPTEST-1**, a gateway of the test post office (RP-3 §9). It carries the session to the post
  office as any gateway does: your messages go in, the waiting ones come down.
* **RPTEST-2**, a client's station (RP-3 §10). It proves itself with its own credential, takes the
  mail addressed to it, and hands you a note.

One session at a time: a call while another is under way is told `busy`. Try again a minute later.

### 1.4 Read the report

The page lists the latest sessions (**All sessions**; `GET /runs.json` for a script). Each report
shows:

| Check | Specification |
|---|---|
| Every PDU framed, in deterministic CBOR, of a known type | RP-3 §2, RP-0 §4.1 |
| The caller's first PDU is HELLO | RP-3 §5.1 |
| Credential, statement and every PDU accepted: no `ERROR` from the service | RP-1 §5, §6.3, RP-3 §5, §7 |
| The session ended with `DONE`, and the caller hung up | RP-3 §5.7, §8 |

Under them, every transmission each way, with its PDUs in CBOR diagnostic notation, and what the
service's station made of the session. A failed check names the transmission and the reason. The
newest 500 reports are kept.

### 1.5 What it does not check yet

The service plays the other side; it does not drive your program. The conformance tester
(`radiopost-tester`, [its outline](../design/01-conformance.md)) goes further on your own computer:
it tells your program what to do, test by test, and checks what it keeps. The test service does
not yet call your gateway, nor check a post office of yours.

---

## Part 2. Hosting the service

### 2.1 What it needs

A Linux server with a public address, `radiopost-test-service` from the release's tools archive,
an HTTPS front (Caddy, nginx) for the page, and one TCP port open for calls (8774).

### 2.2 Running it

```
radiopost-test-service --dir /var/lib/radiopost-test --domain test.radiopost.org \
  --url https://test.radiopost.org --web 127.0.0.1:8630 --air 0.0.0.0:8774
```

* `--dir` holds the test post office (its key and state), the stations' keys, the TLS certificate
  it makes on first start, and the reports. Back it up like any post office's directory.
* `--domain` is the test post office's domain. **Publish nothing for it in DNS** beyond the page's
  address: no `_radiopost` records, so no real gateway or post office ever trusts its credentials.
* `--url` is where the world reaches the page. The test post office's API is under it, at
  `/rp/v1`: logins and enrollments sign that address, so it must be exact.
* `--web` is where the HTTPS front hands requests on. `--air` takes the TCP adapters' calls.

It prints the certificate's fingerprint at start; the page shows it too. A systemd unit keeps it
running:

```
[Unit]
Description=RadioPost test service
After=network-online.target

[Service]
ExecStart=/usr/local/bin/radiopost-test-service --dir /var/lib/radiopost-test --domain test.radiopost.org --url https://test.radiopost.org
Restart=always
DynamicUser=yes
StateDirectory=radiopost-test

[Install]
WantedBy=multi-user.target
```

With `DynamicUser`, `--dir` is `/var/lib/radiopost-test`, the state directory systemd makes.

### 2.3 The HTTPS front

The front hands every request for the service's name to `--web`. With Caddy:

```
test.radiopost.org {
    reverse_proxy 127.0.0.1:8630
}
```

### 2.4 Trying it on one computer

`--plain` makes the links plain TCP, with no certificate, and an empty `--url` makes the page's
address `http://` and `--web`'s. Both are for a test on one computer only.
