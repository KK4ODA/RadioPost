# SPDX-License-Identifier: MIT OR Apache-2.0
"""Email interoperation (RP-6) as a reference: the door, email to message, message to email.

Plain and literal: each function does one section of RP-6 as it is written. DMARC and DKIM
verdicts are inputs, as a mail server hands them over; checking them needs DNS and RSA, which are
the mail server's business, not RadioPost's.
"""

from __future__ import annotations

import base64
import email
import email.policy
import email.quoprimime
import email.utils
import html.parser
import re
import unicodedata
from datetime import UTC, datetime
from email.header import Header
from email.message import EmailMessage
from typing import Any, ClassVar

from . import cbor, rp

# -- §2.1: the door ------------------------------------------------------------------------------

# RP-6 §5.1: who a gateway of each country may hand third-party mail to, by callsign prefix.
OWN_PREFIXES = {"US": ["K", "N", "W", *(f"A{c}" for c in "ABCDEFGHIJKL")]}
ARRANGEMENTS = {  # ARRL, "International Third-Party Traffic", read 2026-11-09
    "US": [
        ("V2", "Antigua and Barbuda"),
        ("LO–LW", "Argentina"),
        ("VK", "Australia"),
        ("V3", "Belize"),
        ("CP", "Bolivia"),
        ("E7", "Bosnia-Herzegovina"),
        ("PP–PY", "Brazil"),
        ("VE, VO, VY", "Canada"),
        ("CA–CE", "Chile"),
        ("HJ–HK", "Colombia"),
        ("D6", "Comoros"),
        ("TI, TE", "Costa Rica"),
        ("CM, CO", "Cuba"),
        ("HI", "Dominican Republic"),
        ("J7", "Dominica"),
        ("HC–HD", "Ecuador"),
        ("YS", "El Salvador"),
        ("C5", "The Gambia"),
        ("9G", "Ghana"),
        ("J3", "Grenada"),
        ("TG", "Guatemala"),
        ("8R", "Guyana"),
        ("HH", "Haiti"),
        ("HQ–HR", "Honduras"),
        ("4X, 4Z", "Israel"),
        ("6Y", "Jamaica"),
        ("JY", "Jordan"),
        ("EL", "Liberia"),
        ("V7", "Marshall Islands"),
        ("XA–XI", "Mexico"),
        ("V6", "Micronesia"),
        ("YN", "Nicaragua"),
        ("HO–HP", "Panama"),
        ("ZP", "Paraguay"),
        ("OA–OC", "Peru"),
        ("DU–DZ", "Philippines"),
        ("V4", "St. Kitts and Nevis"),
        ("J6", "St. Lucia"),
        ("J8", "St. Vincent and the Grenadines"),
        ("9L", "Sierra Leone"),
        ("ZR–ZU", "South Africa"),
        ("3DA", "Eswatini"),
        ("9Y–9Z", "Trinidad and Tobago"),
        ("TA–TC", "Turkey"),
        ("GB", "United Kingdom"),
        ("CV–CX", "Uruguay"),
        ("YV–YY", "Venezuela"),
        ("4U1ITU", "ITU, Geneva"),
        ("4U1VIC", "VIC, Vienna"),
    ]
}
WHOLE_CALLSIGN = re.compile(r"[A-Z0-9]{1,3}[0-9][A-Z]{1,4}")


def prefixes(written: str) -> list[str]:
    """`VE, VO, VY` and `LO–LW` (every prefix between, by the last character) as a list."""
    out = []
    for item in written.split(","):
        first, _, last = item.strip().partition("–")
        if not last:
            out.append(first)
            continue
        out.extend(first[:-1] + chr(c) for c in range(ord(first[-1]), ord(last[-1]) + 1))
    return out


def station_designator(callsign: str) -> str | None:
    """RP-6 §5.1: the part of an operating callsign that says where the station is, if any."""
    parts = callsign.upper().split("/")
    if any(p in {"MM", "AM"} for p in parts):
        return None
    parts = [p for p in parts if p and p not in {"P", "M", "QRP", "A"} and not p.isdigit()]
    designators = [p for p in parts if not WHOLE_CALLSIGN.fullmatch(p)]
    return designators[0] if designators else (parts[0] if parts else None)


def may_hand_third_party(callsign: str, country: str) -> bool:
    """RP-6 §5.1: may a gateway of `country` hand third-party mail to the station `callsign`?"""
    where = station_designator(callsign)
    if where is None:
        return False
    allowed = OWN_PREFIXES.get(country, []) + [
        p for written, _ in ARRANGEMENTS.get(country, []) for p in prefixes(written)
    ]
    return any(where.startswith(p) for p in allowed)


def listed_for(
    entries: list[list[Any]], callsign: str, country: str
) -> tuple[list[list[Any]], int]:
    """RP-6 §5.1: the waiting entries a gateway lists to a station, and how many it holds back."""
    if may_hand_third_party(callsign, country):
        return entries, 0
    listed = [e for e in entries if not e[6] & THIRD_PARTY_FLAG]
    return listed, len(entries) - len(listed)


THIRD_PARTY_FLAG = 8  # RP-3 §5.2


EXECUTABLE_EXTENSIONS = {
    ".exe", ".com", ".scr", ".pif", ".bat", ".cmd", ".msi", ".js", ".jse",
    ".vbs", ".vbe", ".wsf", ".hta", ".jar", ".ps1", ".lnk",
}  # fmt: skip
EXECUTABLE_TYPES = {"application/x-msdownload", "application/x-dosexec", "application/x-msi"}
PGP_MESSAGE = "-----BEGIN PGP MESSAGE-----"

OK_RCPT = "250 2.1.5"
OK_DATA = "250 2.0.0"
NO_MAILBOX = "550 5.1.1"
NOT_LISTED = "550 5.7.1"
TOO_LARGE = "552 5.3.4"
NOT_ALLOWED = "554 5.7.1"


def parse(raw: bytes) -> EmailMessage:
    message = email.message_from_bytes(raw, policy=email.policy.default)
    assert isinstance(message, EmailMessage)
    return message


def domain_of(address: str) -> str:
    return address.rpartition("@")[2].lower()


def on_list(address: str, accept: list[str]) -> bool:
    """§2.2: an address entry matches ignoring case; `@domain` matches that domain exactly."""
    address = address.lower()
    for item in accept:
        entry = item.lower()
        if entry == address or (entry.startswith("@") and entry[1:] == domain_of(address)):
            return True
    return False


def encrypted(message: EmailMessage) -> bool:
    """§2.1: encrypted, as opposed to signed."""
    for part in message.walk():
        kind = part.get_content_type()
        if kind == "multipart/encrypted":
            return True
        enveloped = part.get_param("smime-type", "").lower() != "signed-data"
        if kind in {"application/pkcs7-mime", "application/x-pkcs7-mime"} and enveloped:
            return True
        if part.get_content_maintype() == "text" and PGP_MESSAGE in part.get_content():
            return True
    return False


def executable(message: EmailMessage) -> bool:
    for part in message.walk():
        name = (part.get_filename() or "").lower()
        if part.get_content_type() in EXECUTABLE_TYPES:
            return True
        if any(name.endswith(ext) for ext in EXECUTABLE_EXTENSIONS):
            return True
    return False


def reported_message_id(message: EmailMessage) -> str | None:
    """The `Message-ID` of the message a delivery status report (RFC 3464) is about."""
    if message.get_content_type() != "multipart/report":
        return None
    if message.get_param("report-type", "") != "delivery-status":
        return None
    for part in message.walk():
        kind = part.get_content_type()
        if kind == "text/rfc822-headers":
            original = parse(part.get_payload(decode=True))
            return str(original.get("Message-ID", "")).strip() or None
        if kind == "message/rfc822":
            inner = part.get_payload()[0]
            return str(inner.get("Message-ID", "")).strip() or None
    return None


def door(envelope: dict[str, Any], raw: bytes, ctx: dict[str, Any]) -> dict[str, str | None]:
    """§2.1, in the table's order: the reply to RCPT TO, then to DATA (None if not reached)."""
    sender, recipient = envelope["mail_from"], envelope["rcpt_to"]
    accounts = ctx["accounts"]
    known = [d.lower() for d in ctx["known_post_offices"]]
    if recipient.lower() not in accounts:
        return {"rcpt": NO_MAILBOX, "data": None}
    accept = accounts[recipient.lower()]
    null_sender = not sender
    if not (null_sender or on_list(sender, accept) or domain_of(sender) in known):
        return {"rcpt": NOT_LISTED, "data": None}
    message = parse(raw)
    if ctx["dmarc"] == "fail":
        return {"rcpt": OK_RCPT, "data": NOT_LISTED}
    if null_sender:
        if reported_message_id(message) not in ctx["sent_message_ids"]:
            return {"rcpt": OK_RCPT, "data": NOT_LISTED}
    else:
        header_from = message["From"].addresses[0].addr_spec
        dkim = [d.lower() for d in ctx["dkim_pass"]]
        signed = domain_of(header_from) in known and domain_of(header_from) in dkim
        if not (on_list(header_from, accept) or signed):
            return {"rcpt": OK_RCPT, "data": NOT_LISTED}
    if len(raw) > ctx["max_email_bytes"]:
        return {"rcpt": OK_RCPT, "data": TOO_LARGE}
    if encrypted(message) or executable(message):
        return {"rcpt": OK_RCPT, "data": NOT_ALLOWED}
    return {"rcpt": OK_RCPT, "data": OK_DATA}


# -- §2.4: email to message ----------------------------------------------------------------------


def address(addr: str) -> str:
    """RP-0 §6.2: the domain in lowercase, the local part as written."""
    local, _, domain = addr.rpartition("@")
    return f"{local}@{domain.lower()}"


def text(value: str) -> str:
    """RP-0 §4.3: NFC, LF line endings."""
    return unicodedata.normalize("NFC", value.replace("\r\n", "\n").replace("\r", "\n"))


def tidy(body: str) -> str:
    """§2.4 step 3: no trailing blank lines, and one LF at the end (none if empty)."""
    body = "\n".join(line.rstrip(" \t") for line in body.split("\n")).rstrip("\n")
    return body + "\n" if body else ""


INTRO = re.compile(r"On .* wrote:")
OUTLOOK_RULE = re.compile(r"_{10,}|-----Original Message-----")


def trim(body: str) -> tuple[str, bool]:
    """§2.4 step 4: cut a reply's quoted history at the first line that matches."""
    lines = body.split("\n")
    cut = None
    for i, line in enumerate(lines):
        s = line.strip()
        after = lines[i + 1 :]
        if s == "-----Original Message-----":
            cut = i
        elif line.startswith("From:") and any(n.startswith("Sent:") for n in after[:3]):
            cut = i - 1 if i > 0 and OUTLOOK_RULE.fullmatch(lines[i - 1].strip()) else i
        elif INTRO.fullmatch(s) or (
            s.startswith("On ") and after and INTRO.fullmatch(s + " " + after[0].strip())
        ):
            rest = after if INTRO.fullmatch(s) else after[1:]
            if all(not n.strip() or n.startswith(">") for n in rest):
                cut = i
        if cut is not None:
            break
    if cut is None:
        j = len(lines)
        while j > 0 and (not lines[j - 1].strip() or lines[j - 1].startswith(">")):
            j -= 1
        if any(n.startswith(">") for n in lines[j:]):
            cut = j
    if cut is None:
        return body, False
    return tidy("\n".join(lines[:cut])), True


class _Text(html.parser.HTMLParser):
    """§2.4 step 3: one acceptable rendering of HTML as text."""

    BLOCK: ClassVar = {"p", "div", "br", "li", "tr", "table", "ul", "ol"} | {
        f"h{n}" for n in range(1, 7)
    }

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.out: list[str] = []
        self.links: list[str] = []
        self.href: str | None = None
        self.skip = 0

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        if tag in {"script", "style", "head"}:
            self.skip += 1
        if tag in self.BLOCK:
            self.out.append("\n")
        if tag == "a":
            self.href = dict(attrs).get("href")

    def handle_endtag(self, tag: str) -> None:
        if tag in {"script", "style", "head"}:
            self.skip -= 1
        if tag in {"p", "div", "li", "tr", "table", "ul", "ol"} or tag.startswith("h"):
            self.out.append("\n")
        if tag == "a" and self.href:
            self.links.append(self.href)
            self.out.append(f" [{len(self.links)}]")
            self.href = None

    def handle_data(self, data: str) -> None:
        if not self.skip:
            self.out.append(re.sub(r"\s+", " ", data))

    def render(self) -> str:
        lines = [line.strip() for line in "".join(self.out).split("\n")]
        joined = re.sub(r"\n{3,}", "\n\n", "\n".join(lines)).strip("\n")
        if self.links:
            joined += "\n\n" + "\n".join(f"[{n}] {u}" for n, u in enumerate(self.links, 1))
        return joined + "\n"


def html_to_text(markup: str) -> str:
    parser = _Text()
    parser.feed(markup)
    parser.close()
    return parser.render()


def attachments(message: EmailMessage) -> list[tuple[str, str, bytes]]:
    out = []
    for part in message.iter_attachments():
        data = part.get_payload(decode=True)
        assert isinstance(data, bytes)
        out.append((text(part.get_filename() or "attachment"), part.get_content_type(), data))
    return out


def to_message(raw: bytes, ctx: dict[str, Any]) -> tuple[dict[str, Any], bool]:
    """§2.4: the RadioPost message (named fields, RP-2 §2) an email becomes, and whether the
    text came from HTML (one acceptable rendering, not the only one)."""
    message = parse(raw)
    fields: dict[str, Any] = {"v": 1}
    fields["from"] = address(message["From"].addresses[0].addr_spec)
    for header, name in (("To", "to"), ("Cc", "cc")):
        found = [address(a.addr_spec) for a in message[header].addresses] if message[header] else []
        if found or name == "to":
            fields[name] = found
    try:
        fields["date"] = int(email.utils.parsedate_to_datetime(str(message["Date"])).timestamp())
    except (TypeError, ValueError):
        fields["date"] = ctx["arrival"]
    fields["subject"] = text(str(message.get("Subject", "")))
    plain = message.get_body(preferencelist=("plain",))
    from_html = plain is None
    if plain is not None:
        body = text(plain.get_content())
    else:
        rich = message.get_body(preferencelist=("html",))
        body = text(html_to_text(rich.get_content())) if rich is not None else ""
    body, trimmed = trim(tidy(body))
    fields["body"] = body
    files, held = [], []
    limit = min(ctx["attach_bytes"], ctx["max_air_bytes"])
    found = attachments(message)
    with_files = dict(fields)
    with_files["files"] = [{"name": n, "type": t, "data": d} for n, t, d in found]
    if found and len(rp.air_form(encode_fields(with_files), 0)) <= limit:
        files = with_files["files"]
    else:
        for (n, t, d), ref in zip(found, ctx.get("held_refs", []), strict=False):
            held.append({"name": n, "type": t, "size": len(d), "ref": ref})
    if files:
        fields["files"] = files
    flags = (1 if trimmed else 0) | (2 if held else 0)
    replied = str(message.get("In-Reply-To", "")).strip()
    original = ctx.get("sent_message_ids", {}).get(replied)
    if original is not None:
        fields["reply_to"] = original
    if flags:
        fields["flags"] = flags
    if held:
        fields["held"] = held
    return fields, from_html


def file_map(f: dict[str, Any]) -> dict[int, Any]:
    keys = {"name": 1, "type": 2, "data": 3, "size": 3, "ref": 4}
    return {keys[k]: v for k, v in f.items()}


def encode_fields(fields: dict[str, Any]) -> bytes:
    """A message's canonical encoding (RP-2 §2) from its named fields."""
    out = dict(fields)
    for name in ("files", "held"):
        if name in out:
            out[name] = [file_map(f) for f in out[name]]
    return cbor.encode(rp.message_map(out))


# -- §3: message to email ------------------------------------------------------------------------

FOOTER = (
    "-- \n"
    "Sent by amateur radio through RadioPost. Please reply in plain text, under 20 kB.\n"
    "Amateur radio carries no business content.\n"
)


def rfc5322_date(seconds: int) -> str:
    """§3.1: RFC 5322 date-time in UTC, written `+0000`."""
    return email.utils.format_datetime(datetime.fromtimestamp(seconds, UTC))


def iso(seconds: int) -> str:
    return datetime.fromtimestamp(seconds, UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


def encoded_header(value: str) -> str:
    """RFC 2047 when the value is not ASCII; one valid spelling of it."""
    if value.isascii():
        return value
    return Header(value, "utf-8").encode(linesep="\r\n")


def display(name: str, addr: str) -> str:
    if not name:
        return addr
    if name.isascii() and re.fullmatch(r"[A-Za-z0-9 !#$%&'*+/=?^_`{|}~-]+", name):
        return f"{name} <{addr}>"
    if name.isascii():
        return '"' + name.replace("\\", "\\\\").replace('"', '\\"') + f'" <{addr}>'
    return f"{encoded_header(name)} <{addr}>"


def body_part(body: str) -> tuple[str, str]:
    """§3.1: the body and footer as a text part: its transfer encoding and its CRLF text."""
    full = (body if body.endswith("\n") or not body else body + "\n") + FOOTER
    long_line = any(len(line.encode("utf-8")) > 998 for line in full.split("\n"))
    if full.isascii() and not long_line:
        return "7bit", full.replace("\n", "\r\n")
    as_bytes = full.encode("utf-8").decode("latin-1")  # body_encode reads one byte a character
    encoded = email.quoprimime.body_encode(as_bytes, maxlinelen=76, eol="\r\n")
    return "quoted-printable", encoded if encoded.endswith("\r\n") else encoded + "\r\n"


def radio_path(path: dict[str, Any]) -> str:
    """§3.2."""
    via = " via ".join([path["operator"], *path["gateways"]])
    return f"{via}; at={iso(path['started'])}; session={path['session']}"


def part_bytes(proof: dict[str, Any]) -> bytes:
    """§4: the `application/radiopost-message` part's CBOR map."""
    return cbor.encode(
        {
            1: proof["air"],
            2: proof["credential"],
            3: proof["statement"],
            4: proof["signature"],
            5: proof["started"],
        }
    )


def from_message(fields: dict[str, Any], ctx: dict[str, Any]) -> tuple[dict[str, str], bytes]:
    """§3.1: the email a message handed in by radio becomes, before DKIM signing.

    Returns its headers by name (as written) and the whole email in CRLF.
    """
    canonical = encode_fields(fields)
    cid = rp.id_text(rp.content_id(canonical))
    domain = domain_of(fields["from"])
    headers: dict[str, str] = {
        "From": display(ctx["display_name"], fields["from"]),
        "To": ", ".join(fields["to"]),
    }
    if fields.get("cc"):
        headers["Cc"] = ", ".join(fields["cc"])
    headers["Date"] = rfc5322_date(fields["date"])
    headers["Subject"] = encoded_header(fields["subject"])
    headers["Message-ID"] = f"<{cid}@{domain}>"
    replied = ctx.get("message_ids", {}).get(fields.get("reply_to", b""))
    if replied:
        headers["In-Reply-To"] = replied
        headers["References"] = replied
    headers["Radio-Path"] = radio_path(ctx["radio_path"])
    if fields.get("priority", 0) >= 1:
        headers["Importance"] = "high"
    headers["MIME-Version"] = "1.0"
    encoding, body = body_part(fields["body"])
    proof = ctx.get("proof")
    if proof is None and not fields.get("files"):
        headers["Content-Type"] = "text/plain; charset=utf-8"
        headers["Content-Transfer-Encoding"] = encoding
        head = "".join(f"{k}: {v}\r\n" for k, v in headers.items())
        return headers, (head + "\r\n" + body).encode("utf-8")
    boundary = f"radiopost-{cid}"
    headers["Content-Type"] = f'multipart/mixed; boundary="{boundary}"'
    parts = [
        "Content-Type: text/plain; charset=utf-8\r\n"
        f"Content-Transfer-Encoding: {encoding}\r\n\r\n{body}"
    ]
    for f in fields.get("files", []):
        parts.append(
            f"Content-Type: {f['type']}\r\n"
            f'Content-Disposition: attachment; filename="{f["name"]}"\r\n'
            "Content-Transfer-Encoding: base64\r\n\r\n" + base64_lines(f["data"])
        )
    if proof is not None:
        parts.append(
            "Content-Type: application/radiopost-message\r\n"
            "Content-Transfer-Encoding: base64\r\n\r\n" + base64_lines(part_bytes(proof))
        )
    head = "".join(f"{k}: {v}\r\n" for k, v in headers.items())
    # RFC 2046 §5.1.1: the CRLF before a boundary belongs to the boundary, so each part keeps
    # its own last line break only with one more.
    middle = "".join(f"--{boundary}\r\n{p}\r\n" for p in parts)
    return headers, (head + "\r\n" + middle + f"--{boundary}--\r\n").encode("utf-8")


def base64_lines(data: bytes) -> str:
    b64 = base64.b64encode(data).decode("ascii")
    return "".join(b64[i : i + 76] + "\r\n" for i in range(0, len(b64), 76))


# -- §4: the message part, at the receiving post office ------------------------------------------


def check_part(
    part: bytes,
    recipient: str,
    post_offices: dict[str, list[dict[str, Any]]],
    revoked: dict[str, dict[str, list[Any]]],
) -> tuple[str, bytes | None]:
    """§4: ("ok", the canonical encoding) or (the RP-0 §9.3 name of what failed, None).

    The proof is checked as RP-1 §6.3 says for a statement relayed later (steps 1, 2 and 4, and
    `time` against `started`), then the content ID, the sender, and that the message is for
    `recipient`, the address this SMTP transaction delivers to.
    """
    try:
        m = cbor.decode(part)
        air, credential, statement_raw, signature, started = (m[k] for k in range(1, 6))
        canonical = rp.air_decode(air)
        stmt = cbor.decode(statement_raw)
    except (cbor.NotCanonical, ValueError, KeyError, TypeError):
        return "malformed", None
    result, body = rp.verify_credential(credential, post_offices, revoked, started, ["send"])
    if result != "ok" or body is None:
        return result, None
    if stmt.get(1) != body[3] or stmt.get(2) != body[4]:
        return "not-authorized", None
    if not rp.verify(rp.CTX_SESSION, statement_raw, signature, body[5]):
        return "signature-invalid", None
    if 4 in stmt and abs(stmt[4] - started) > rp.TIME_WINDOW_S:
        return "time-skew", None
    if rp.content_id(canonical) not in stmt.get(6, []):
        return "not-authorized", None
    message = cbor.decode(canonical)
    if message.get(2) != stmt[1]:
        return "not-authorized", None
    if recipient.lower() not in [a.lower() for a in message.get(3, []) + message.get(4, [])]:
        return "not-authorized", None
    return "ok", canonical


def find_part(raw: bytes) -> bytes | None:
    for part in parse(raw).walk():
        if part.get_content_type() == "application/radiopost-message":
            data = part.get_payload(decode=True)
            assert isinstance(data, bytes)
            return data
    return None


# -- §3.3: bounces -------------------------------------------------------------------------------


def bounce(raw: bytes, ctx: dict[str, Any]) -> dict[str, Any]:
    """§3.3: the short message a delivery failure report becomes in the sender's mailbox."""
    report = parse(raw)
    lines, original_subject, original_id = [], "", None
    for part in report.walk():
        kind = part.get_content_type()
        if kind == "message/delivery-status":
            for block in part.get_payload()[1:]:
                if str(block.get("Action", "")).strip().lower() != "failed":
                    continue
                failed = str(block["Final-Recipient"]).partition(";")[2].strip()
                diagnostic = str(block.get("Diagnostic-Code", "")).partition(";")[2].strip()
                reason = diagnostic or str(block.get("Status", "")).strip()
                lines.append(f"{failed}: {reason}")
        if kind == "text/rfc822-headers":
            original = parse(part.get_payload(decode=True))
            original_subject = str(original.get("Subject", ""))
            original_id = str(original.get("Message-ID", "")).strip()
    fields: dict[str, Any] = {
        "v": 1,
        "from": "postmaster@" + ctx["domain"],
        "to": [ctx["sender"]],
        "date": ctx["arrival"],
        "subject": text("Not delivered: " + original_subject),
        "body": "\n".join(lines) + "\n",
    }
    original_cid = ctx["sent_message_ids"].get(original_id or "")
    if original_cid is not None:
        fields["reply_to"] = original_cid
    return fields
