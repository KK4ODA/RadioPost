# SPDX-License-Identifier: MIT OR Apache-2.0
"""Draw the schematics of the post office server guide (docs/user/post-office.md).

Run from anywhere: ``python tools/figures/server.py``. It writes one SVG per figure into
docs/user/fig/. Like the manual's, the figures carry no number or title: the guide's caption
under each one does. The guide's screenshots of the desk are the manual's.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from manual import LOCAL, Plate
from style import HAIR, INK, LEGEND, LINKS, MUTED, ROLES

OUT = Path(__file__).resolve().parents[2] / "docs" / "user" / "fig"


def port(f: Plate, x: float, y: float, text: str) -> None:
    """A small label naming a port, centred on ``x``."""
    f.badge(x, y, text, "paper", size=11)


# ------------------------------------------------------------------------------------------------
# What runs on the server
# ------------------------------------------------------------------------------------------------


def fig_overview() -> None:
    f = Plate(1000, 490, "What runs on a post office's server")
    f.computer(268, 20, 464, 404, "Your server, po.radiopost.org")
    f.box(
        292,
        58,
        196,
        124,
        "neutral",
        tag="Web server",
        title="Caddy",
        rows=["The certificate, and", "HTTPS for the post", "office and webmail"],
        icon="globe",
    )
    f.box(
        512,
        58,
        196,
        124,
        "po",
        tag="The post office",
        title="radiopost-po",
        rows=["Accounts, the radio", "mailboxes, the desk,", "the door for email"],
        icon="envelope",
    )
    f.box(
        292,
        226,
        196,
        124,
        "mail",
        tag="Mail server",
        title="Stalwart",
        rows=["Email in and out, the", "mailboxes users read", "in mail programs"],
        icon="server",
    )
    f.box(
        512,
        226,
        196,
        124,
        "neutral",
        tag="Kept on the server",
        title="Jobs and backups",
        rows=["The desk's jobs, run", "from a fixed list;", "nightly backups"],
        icon="doc",
    )
    f.link([(492, 120), (508, 120)], "local", head=False)
    f.link([(390, 186), (390, 222)], "local", head=False)
    f.link([(560, 186), (465, 222)], "local", both=True)
    f.text(
        585,
        210,
        "the door, and email out",
        size=11.5,
        weight=600,
        color=LINKS["local"][0],
        halo=True,
    )
    f.link([(680, 186), (680, 222)], "local", head=False)
    f.text(
        292, 380, "Open to the Internet: 22 (SSH), 25, 80, 443, 465 and 993.", size=12, color=MUTED
    )
    f.text(292, 398, "Everything else stays on the server.", size=12, color=MUTED)

    # outside, on the left: RadioPost
    f.box(
        24,
        40,
        200,
        96,
        "client",
        tag="Users and gateways",
        title="RadioPost",
        rows=["clients, gateways"],
        icon="antenna",
    )
    f.box(
        24,
        160,
        200,
        96,
        "client",
        tag="You, the operator",
        title="The desk",
        rows=["in your RadioPost"],
        icon="person",
    )
    f.box(
        24,
        280,
        200,
        96,
        "neutral",
        tag="Users, by email too",
        title="Mail programs",
        rows=["Outlook, Thunderbird…"],
        icon="envelope",
    )
    f.link([(228, 88), (288, 100)], "internet", both=True)
    f.link([(228, 208), (288, 140)], "internet", both=True)
    port(f, 258, 172, "443")
    f.link([(228, 328), (288, 290)], "smtp", both=True)
    port(f, 258, 330, "465, 993")

    # outside, on the right: the world's email and DNS
    f.box(
        776,
        200,
        200,
        112,
        "mail",
        tag="The world's email",
        title="Gmail, Outlook…",
        rows=["Mail servers", "everywhere"],
        icon="globe",
    )
    f.link([(772, 262), (712, 262)], "smtp", both=True)
    port(f, 742, 290, "25")
    f.box(
        776,
        40,
        200,
        112,
        "neutral",
        tag="At your DNS host",
        title="DNS records",
        rows=["How the world finds", "and trusts the server"],
        icon="globe",
    )
    f.legend(476, [*LEGEND[1:], LOCAL])
    f.save(OUT / "overview.svg")


# ------------------------------------------------------------------------------------------------
# The setup, step by step
# ------------------------------------------------------------------------------------------------

STEPS = [
    ("1", "Updates and firewall", ["Security updates nightly;", "only six ports open"], False),
    ("2", "The server's name", ["po.<domain> in DNS,", "reverse DNS set"], True),
    ("3", "SSH by key only", ["Password logins", "turned off"], False),
    ("4", "Caddy", ["The web server and", "its certificate"], False),
    ("5", "Stalwart", ["The mail server: its", "setup page, by you"], True),
    ("6", "The post office", ["radiopost-po, its", "service and jobs"], False),
    ("7", "DNS", ["MX, SPF, DKIM, DMARC", "and RadioPost's records"], True),
    ("8", "The check", ["Everything, seen", "from outside"], False),
]


def fig_roadmap() -> None:
    f = Plate(1000, 420, "Setting up a post office")
    f.box(
        24,
        20,
        952,
        64,
        "client",
        tag="Before: chapter 4",
        title="Rent a Debian server, log in with an SSH key, check port 25",
        title_size=14,
    )
    orange = ROLES["gateway"][1]
    w, h, gap = 220, 104, 24
    for i, (n, title, rows, by_hand) in enumerate(STEPS):
        col, row = i % 4, i // 4
        x = 24 + col * (w + gap)
        y = 112 + row * (h + 28)
        f.box(x, y, w, h, "gateway" if by_hand else "neutral", title=title, rows=rows)
        f.number(x + w - 22, y + 22, n, orange if by_hand else INK)
        if col < 3:
            f.link([(x + w + 4, y + h / 2), (x + w + gap - 4, y + h / 2)], "local")
    f.link([(24 + 3 * (w + gap) + w / 2, 112 + h + 2), (24 + w / 2, 112 + h + 26)], "local")
    f.box(
        24,
        372,
        952,
        40,
        "po",
        title="After: chapters 6 and 7. Enroll your own RadioPost, open the desk, test, go live.",
        title_size=14,
    )
    # the key
    f.rect(620, 92, 14, 14, ROLES["gateway"][0], orange, rx=3, sw=1.4)
    f.text(640, 104, "You do part of it by hand: the installer says what, and waits", size=12)
    f.save(OUT / "roadmap.svg")


# ------------------------------------------------------------------------------------------------
# How mail comes in and goes out
# ------------------------------------------------------------------------------------------------


def fig_mail() -> None:
    f = Plate(1000, 440, "How email comes in and goes out")
    f.text(24, 30, "Coming in", size=15, weight=700, color=INK)
    y, h = 46, 124
    f.box(
        24,
        y,
        200,
        h,
        "mail",
        tag="Someone writes",
        title="An email",
        rows=["to ana@radiopost.org,", "from any provider"],
        icon="globe",
    )
    f.box(
        272,
        y,
        200,
        h,
        "mail",
        tag="Port 25",
        title="Stalwart",
        rows=["takes the email, and", "asks the door first"],
        icon="server",
    )
    f.box(
        520,
        y,
        200,
        h,
        "po",
        tag="The door",
        title="Let it in?",
        rows=["On the accept list?", "SPF, DKIM, DMARC?", "Not encrypted?"],
    )
    f.box(
        768,
        y,
        208,
        h,
        "client",
        tag="Kept twice",
        title="For the radio",
        rows=["and in the mailbox", "a mail program reads"],
        icon="antenna",
    )
    mid = y + h / 2
    f.link([(228, mid), (268, mid)], "smtp")
    f.link([(476, mid), (516, mid)], "local")
    f.link([(724, mid), (764, mid)], "local")
    f.text(
        620,
        y + h + 22,
        "No: refused, with the reason",
        size=12,
        color=ROLES["warn"][1],
        anchor="middle",
    )

    f.text(24, 234, "Going out", size=15, weight=700, color=INK)
    y2 = 250
    f.box(
        24,
        y2,
        200,
        h,
        "client",
        tag="A user, by radio",
        title="Through a gateway",
        rows=["the gateway passes", "the session on"],
        icon="antenna",
    )
    f.box(
        272,
        y2,
        200,
        h,
        "po",
        tag="The post office",
        title="Makes the email",
        rows=["from the user's address,", "with a Radio-Path line"],
        icon="envelope",
    )
    f.box(
        520,
        y2,
        200,
        h,
        "mail",
        tag="Port 465, on the server",
        title="Stalwart",
        rows=["signs it (DKIM)", "and sends it"],
        icon="server",
    )
    f.box(
        768,
        y2,
        208,
        h,
        "mail",
        tag="Anyone",
        title="Email arrives",
        rows=["in a minute or so"],
        icon="globe",
    )
    mid2 = y2 + h / 2
    f.link([(228, mid2), (268, mid2)], "internet")
    f.link([(476, mid2), (516, mid2)], "local")
    f.link([(724, mid2), (764, mid2)], "smtp")
    f.legend(430, [*LEGEND[1:], LOCAL])
    f.save(OUT / "mail.svg")


# ------------------------------------------------------------------------------------------------
# DNS: the records, and who reads them
# ------------------------------------------------------------------------------------------------

RECORDS = [
    ("po.radiopost.org", "A, AAAA", "The server's address", "all"),
    ("radiopost.org", "MX", "Where email for the domain goes", "mail"),
    ("radiopost.org", "TXT (SPF)", "Which servers may send its email", "mail"),
    ("po.radiopost.org", "TXT (SPF)", "The mail host itself may send", "mail"),
    ("v1-rsa-…._domainkey", "TXT (DKIM)", "A key its email is signed with", "mail"),
    ("v1-ed25519-…._domainkey", "TXT (DKIM)", "The second key", "mail"),
    ("_dmarc.radiopost.org", "TXT (DMARC)", "What to do with mail that fails", "mail"),
    ("_radiopost.radiopost.org", "TXT", "The post office's API and key", "rp"),
    ("(the server's address)", "PTR", "Reverse DNS: the address's name", "mail"),
]


def fig_dns() -> None:
    f = Plate(1000, 384, "The DNS records and who reads them")
    f.panel(24, 20, 952, 350, "The records a post office publishes")
    for x, head in [(40, "Name"), (292, "Type"), (420, "What it says"), (760, "Read by")]:
        f.text(x, 64, head, size=12, weight=700, color=MUTED)
    f.add(f'<path d="M 40 74 L 960 74" stroke="{HAIR}" stroke-width="1"/>')
    readers = {
        "all": ("Everyone", "neutral"),
        "mail": ("Mail servers", "mail"),
        "rp": ("RadioPost programs", "client"),
    }
    y = 100
    for name, kind, what, who in RECORDS:
        f.text(40, y, name, size=12.5, mono=True)
        f.text(292, y, kind, size=12.5, weight=700, color=ROLES["po"][1])
        f.text(420, y, what, size=12.5, color=INK)
        label, role = readers[who]
        f.badge(760 + len(label) * 3.4 + 8, y, label, role, size=11.5)
        y += 30
    f.save(OUT / "dns.svg")


# ------------------------------------------------------------------------------------------------
# Backups
# ------------------------------------------------------------------------------------------------


def fig_backups() -> None:
    f = Plate(1000, 330, "Backups: made on the server, kept off it")
    f.box(
        24,
        40,
        280,
        150,
        "po",
        tag="Each night, on the server",
        title="Two backups",
        rows=[
            "The post office: its key,",
            "accounts and state (7 kept)",
            "The mailboxes, Stalwart",
            "stopped a moment (3 kept)",
        ],
        icon="server",
    )
    f.box(
        396,
        20,
        280,
        112,
        "client",
        tag="By itself, while RadioPost is open",
        title="Your computer",
        rows=["fetches each new one,", "sealed to its own key"],
        icon="person",
    )
    f.box(
        396,
        152,
        280,
        112,
        "client",
        tag="By hand, from the desk",
        title="Download a backup",
        rows=["sealed with a passphrase", "you choose"],
        icon="key",
    )
    f.box(
        768,
        86,
        208,
        112,
        "neutral",
        tag="If the server is lost",
        title="Restore",
        rows=["on a new server,", "mailboxes and all"],
        icon="doc",
    )
    f.link([(308, 100), (392, 76)], "internet")
    f.link([(308, 130), (392, 208)], "internet")
    f.link([(680, 76), (764, 130)], "local")
    f.link([(680, 208), (764, 156)], "local")
    f.lines(
        24,
        294,
        [
            "A backup that stays on the server is lost with it. The desk's Health turns amber 7"
            " days after a copy last left the server, and red after 30.",
        ],
        size=12.5,
        color=MUTED,
    )
    f.save(OUT / "backups.svg")


# ------------------------------------------------------------------------------------------------
# A backup mail server
# ------------------------------------------------------------------------------------------------


def fig_backup_mx() -> None:
    f = Plate(1000, 360, "A backup mail server")
    red = ROLES["warn"][1]
    f.box(
        24,
        110,
        210,
        112,
        "mail",
        tag="Someone writes",
        title="Their mail server",
        rows=["tries MX 10 first,", "then MX 20"],
        icon="globe",
    )
    f.box(
        396,
        20,
        250,
        112,
        "po",
        tag="MX 10: your post office",
        title="po.radiopost.org",
        rows=["down for a while", "(an outage, an update)"],
        icon="envelope",
    )
    f.box(
        396,
        200,
        250,
        124,
        "mail",
        tag="MX 20: the backup",
        title="mx2.radiopost.org",
        rows=[
            "Stalwart and its own door,",
            "judging with a copy of the",
            "accounts and accept lists",
        ],
        icon="server",
    )
    f.link([(238, 150), (300, 92), (392, 76)], "fail")
    f.cross(318, 82, red)
    f.link([(238, 190), (392, 262)], "smtp", label="Takes the mail", at=0.55, dy=-10)
    f.link(
        [(650, 262), (800, 262), (800, 76), (650, 76)],
        "smtp",
        label=["Hands it on when", "the post office is back,", "its verdicts sealed"],
        at=0.5,
        dx=12,
        anchor="start",
        size=12,
    )
    f.link(
        [(560, 136), (560, 196)],
        "internet",
        label=["A copy every", "five minutes"],
        at=0.5,
        dx=10,
        dy=-2,
        anchor="start",
        size=12,
    )
    f.legend(352, [*LEGEND[1:], ("fail", "Not answering")])
    f.save(OUT / "backup-mx.svg")


FIGURES = [fig_overview, fig_roadmap, fig_mail, fig_dns, fig_backups, fig_backup_mx]


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    for draw in FIGURES:
        draw()
    print(f"{len(FIGURES)} figures in {OUT}")


if __name__ == "__main__":
    main()
