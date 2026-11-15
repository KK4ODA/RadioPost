# SPDX-License-Identifier: MIT OR Apache-2.0
"""Draw the schematics of the RadioPost manual (docs/manual/manual.md).

Run from anywhere: ``python tools/figures/manual.py``. It writes one SVG per figure into
docs/manual/fig/. The figures carry no number or title of their own: the manual's caption under
each one does, so a figure can move without being redrawn. The screenshots beside them come from
tools/manual/screens.mjs.
"""

from __future__ import annotations

import sys
from pathlib import Path
from xml.sax.saxutils import escape

sys.path.insert(0, str(Path(__file__).parent))

from style import FAINT, HAIR, INK, LEGEND, LINKS, MUTED, PAPER, ROLES, Fig

OUT = Path(__file__).resolve().parents[2] / "docs" / "manual" / "fig"

LOCAL = ("local", "On the same computer")


class Plate(Fig):
    """A figure with no title drawn on it: the manual's caption names it."""

    def svg(self) -> str:
        defs = "".join(
            f'<marker id="ah-{kind}" viewBox="0 0 10 10" refX="8.5" refY="5" '
            'markerWidth="7" markerHeight="7" orient="auto-start-reverse">'
            f'<path d="M 0 0 L 10 5 L 0 10 z" fill="{color}"/></marker>'
            for kind, (color, _dash, _w) in LINKS.items()
        )
        head = [
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {self.w} {self.h}" '
            f'width="{self.w}" height="{self.h}" role="img" aria-label="{escape(self.title)}">',
            f"<defs>{defs}</defs>",
            f'<rect x="0" y="0" width="{self.w}" height="{self.h}" fill="{PAPER}"/>',
        ]
        return "\n".join([*head, *self.body, "</svg>", ""])

    def computer(self, x: float, y: float, w: float, h: float, label: str) -> None:
        """A dashed frame round what runs on one computer, named at its top left."""
        self.rect(x, y, w, h, "#f8fafc", FAINT, rx=12, sw=1.3, dash="6 5")
        self.text(x + 14, y + 22, label, size=12.5, weight=700, color=MUTED)

    def note(self, x: float, y: float, rows: list[str], *, size: float = 12.5) -> float:
        return self.lines(x, y, rows, size=size, color=MUTED)

    def step(self, x: float, y: float, n: str, rows: list[str], color: str = INK) -> None:
        """A numbered marker with its text to the right."""
        self.number(x, y, n, color)
        self.lines(x + 20, y + 4.5, rows, size=12.5, color=INK)


# ------------------------------------------------------------------------------------------------
# How RadioPost works, in one line
# ------------------------------------------------------------------------------------------------


def fig_overview() -> None:
    f = Plate(1000, 300, "How RadioPost carries your email")
    y, h, w = 40, 112, 196
    xs = [24, 282, 540, 780]
    f.box(
        xs[0],
        y,
        w,
        h,
        "client",
        tag="Your station",
        title="RadioPost",
        rows=["The client program, with", "Aether HF and your radio"],
        icon="person",
    )
    f.box(
        xs[1],
        y,
        w,
        h,
        "gateway",
        tag="A gateway",
        title="KK4ODA-1",
        rows=["Another amateur's station,", "on the Internet"],
        icon="antenna",
    )
    f.box(
        xs[2],
        y,
        w - 20,
        h,
        "po",
        tag="Your post office",
        title="radiopost.org",
        rows=["Keeps your mailbox,", "sends and takes email"],
        icon="envelope",
    )
    f.box(
        xs[3],
        y,
        w,
        h,
        "mail",
        tag="Anyone with email",
        title="Gmail, Outlook…",
        rows=["Writes to you and gets", "your answers as usual"],
        icon="globe",
    )
    mid = y + h / 2
    f.link([(xs[0] + w + 4, mid), (xs[1] - 4, mid)], "radio", both=True, label="By radio")
    f.link([(xs[1] + w + 4, mid), (xs[2] - 4, mid)], "internet", both=True, label="Internet")
    f.link([(xs[2] + w - 16, mid), (xs[3] - 4, mid)], "smtp", both=True, label="Email")

    f.panel(24, 186, 952, 74, "In short")
    f.note(
        38,
        232,
        [
            "Your address is an ordinary email address: name@radiopost.org. Mail for you waits at"
            " your post office until you call a gateway.",
        ],
    )
    f.note(
        38,
        250,
        [
            "Any gateway takes your call, whoever runs it. Two RadioPost stations can also send"
            " mail to each other directly, with no gateway.",
        ],
    )
    f.legend(288, LEGEND)
    f.save(OUT / "overview.svg")


# ------------------------------------------------------------------------------------------------
# Your station: the programs, the interface, the radio
# ------------------------------------------------------------------------------------------------


def fig_station() -> None:
    f = Plate(1000, 330, "Your station: what connects to what")
    f.computer(24, 24, 500, 200, "Your computer")
    f.box(
        44,
        58,
        210,
        142,
        "client",
        tag="Program 1",
        title="RadioPost",
        rows=["Your mail: write, read,", "call a gateway or", "another station"],
        icon="envelope",
    )
    f.box(
        296,
        58,
        210,
        142,
        "neutral",
        tag="Program 2",
        title="Aether HF",
        rows=["The modem: turns data", "into audio, keys the", "radio, checks the rules"],
        icon="antenna",
    )
    f.link(
        [(258, 129), (292, 129)],
        "local",
        both=True,
        label=["8515"],
        dy=-10,
    )
    f.box(
        572,
        58,
        180,
        142,
        "neutral",
        tag="Interface",
        title="Audio and PTT",
        rows=["A USB sound card", "(SignaLink, DigiRig)", "or the radio's own USB"],
    )
    f.box(
        800,
        58,
        176,
        142,
        "neutral",
        tag="Radio",
        title="HF transceiver",
        rows=["Upper sideband,", "set for data, with", "its antenna"],
        icon="antenna",
    )
    f.link([(528, 129), (568, 129)], "local", both=True, label="USB", dy=-10)
    f.link([(756, 129), (796, 129)], "local", both=True, label="Audio", dy=-10)

    f.panel(24, 244, 952, 72, "What you set where")
    f.note(
        38,
        288,
        [
            "RadioPost: your account and your mail. Aether HF: the radio, the sound card, keying"
            " and the band rules (its own manual).",
        ],
    )
    f.note(
        38,
        306,
        [
            "RadioPost finds Aether at ws://127.0.0.1:8515/v1. You do not copy your callsign into"
            " Aether: RadioPost tells it.",
        ],
    )
    f.save(OUT / "station.svg")


# ------------------------------------------------------------------------------------------------
# Getting an account: a code, or an application
# ------------------------------------------------------------------------------------------------


def fig_joining() -> None:
    f = Plate(1000, 400, "Two ways to get an account")
    blue = ROLES["client"][1]
    green = ROLES["po"][1]
    f.panel(24, 20, 600, 168, "With a code from the post office's operator")
    f.step(48, 66, "1", ["You email the operator your callsign and the", "address you want."])
    f.step(
        48,
        112,
        "2",
        ["The operator checks your licence and sends a", "one-time code, good for 7 days."],
    )
    f.step(48, 160, "3", ["In RadioPost's Setup: I have a code, then Enroll."], blue)

    f.panel(24, 208, 600, 172, "With an application")
    f.step(48, 254, "1", ["In RadioPost's Setup: Apply for an account, then Apply."], blue)
    f.step(
        48,
        298,
        "2",
        [
            "The operators check your licence and decide:",
            "approve, decline, or send a code by post.",
        ],
    )
    f.step(
        48,
        346,
        "3",
        [
            "RadioPost asks every minute while it is open,",
            "and takes the credential once approved.",
        ],
        blue,
    )

    f.box(
        700,
        124,
        276,
        152,
        "po",
        tag="You end with",
        title="A credential",
        rows=[
            "Your address and callsign,",
            "signed by your post office.",
            "Good for 90 days; RadioPost",
            "renews it in its last month.",
        ],
        icon="key",
    )
    f.link([(628, 104), (696, 170)], "internet")
    f.link([(628, 294), (696, 230)], "internet")
    f.lines(
        700,
        304,
        ["Both ways go over the Internet, once.", "Blue numbers are what you do in RadioPost."],
        size=12.5,
        color=green,
    )
    f.save(OUT / "joining.svg")


# ------------------------------------------------------------------------------------------------
# A message out, and the answer back
# ------------------------------------------------------------------------------------------------


def fig_journey() -> None:
    f = Plate(1000, 430, "A message out, and the answer back")
    f.text(24, 30, "Sending", size=15, weight=700, color=INK)
    y, h = 46, 104
    for x, w, role, tag, title, rows in [
        (24, 210, "client", "You", "Write", ["Put in outbox: it waits", "on this computer"]),
        (
            276,
            210,
            "client",
            "You",
            "Call a gateway",
            ["The session carries", "the outbox by radio"],
        ),
        (
            528,
            210,
            "gateway",
            "The gateway",
            "Passes it on",
            ["to your post office,", "over the Internet"],
        ),
        (
            780,
            196,
            "mail",
            "The other end",
            "Email arrives",
            ["from your address,", "in a minute or so"],
        ),
    ]:
        f.box(x, y, w, h, role, tag=tag, title=title, rows=rows)
    mid = y + h / 2
    f.link([(238, mid), (272, mid)], "local")
    f.link([(490, mid), (524, mid)], "radio")
    f.link([(742, mid), (776, mid)], "internet")

    f.text(24, 206, "Receiving", size=15, weight=700, color=INK)
    y2 = 222
    for x, w, role, tag, title, rows in [
        (
            24,
            210,
            "mail",
            "The other end",
            "Answers",
            ["an ordinary email to", "name@radiopost.org"],
        ),
        (
            276,
            210,
            "po",
            "Your post office",
            "Keeps it",
            ["until you call; copies", "it to usual gateways"],
        ),
        (528, 210, "client", "You", "Call a gateway", ["Any gateway; the", "session fetches it"]),
        (780, 196, "client", "You", "Inbox", ["Read, reply,", "forward"]),
    ]:
        f.box(x, y2, w, h, role, tag=tag, title=title, rows=rows)
    mid2 = y2 + h / 2
    f.link([(238, mid2), (272, mid2)], "smtp")
    f.link([(490, mid2), (524, mid2)], "internet")
    f.link([(742, mid2), (776, mid2)], "radio")

    f.panel(24, 350, 952, 40, "One call does both")
    f.note(
        160,
        374,
        [
            "Each call to a gateway sends your outbox and fetches what waits for you;"
            " Fetch, beside Call, says how much.",
        ],
    )
    f.legend(420, LEGEND)
    f.save(OUT / "journey.svg")


# ------------------------------------------------------------------------------------------------
# Station to station
# ------------------------------------------------------------------------------------------------


def fig_direct() -> None:
    f = Plate(1000, 310, "Station to station")
    f.box(
        24,
        30,
        260,
        112,
        "client",
        tag="Station A calls",
        title="facundo@radiopost.org",
        rows=["Another client's station,", "N0CALL, then Call"],
        icon="person",
        title_size=14.5,
    )
    f.box(
        716,
        30,
        260,
        112,
        "client",
        tag="Station B listens",
        title="ana@hampost.org",
        rows=["Listen: it answers", "one station's call"],
        icon="person",
        title_size=14.5,
    )
    f.link(
        [(288, 86), (712, 86)],
        "radio",
        both=True,
        label=["Mail both ways, each station checked", "by its credential: no Internet needed"],
        dy=-12,
    )
    f.box(
        360,
        168,
        280,
        104,
        "po",
        tag="Later, with the Internet",
        title="Each post office",
        rows=["is told what crossed:", "Send reports"],
        icon="envelope",
    )
    f.link([(154, 146), (154, 220), (356, 220)], "internet")
    f.link([(846, 146), (846, 220), (644, 220)], "internet")
    f.legend(302, LEGEND[:2])
    f.save(OUT / "direct.svg")


# ------------------------------------------------------------------------------------------------
# Several callsigns on one computer
# ------------------------------------------------------------------------------------------------


def fig_callsigns() -> None:
    f = Plate(1000, 340, "Several callsigns on one computer")
    f.computer(24, 20, 952, 300, "One computer, one RadioPost")
    # the list at the top
    f.rect(48, 52, 420, 40, PAPER, HAIR, rx=8)
    f.text(64, 77, "N0CALL  ·  ana@radiopost.org", size=13.5, weight=600, mono=True)
    f.text(452, 78, "▾", size=14, color=MUTED, anchor="end")
    f.text(484, 70, "The list at the top of the window", size=12.5, weight=700, color=INK)
    f.text(484, 88, "chooses which callsign you work as.", size=12.5, color=MUTED)

    homes = [
        ("First callsign", "ana@radiopost.org", "N0CALL"),
        ("Added", "club@radiopost.org", "N1CALL"),
        ("Added", "Applied for", "N2CALL"),
    ]
    for i, (tag, title, call) in enumerate(homes):
        x = 48 + i * 236
        f.box(
            x,
            118,
            220,
            110,
            "client",
            tag=f"{tag} · {call}",
            title=title,
            rows=["Its own account,", "credential and mail"],
            icon="key",
            title_size=14.5,
        )
    f.box(
        760,
        118,
        196,
        110,
        "neutral",
        tag="Shared by all",
        title="This computer's",
        rows=["device key, address", "list and log"],
    )
    f.note(
        48,
        258,
        [
            "A call goes out as the callsign chosen, and Listen answers that callsign only.",
            "Change it between sessions. Every credential renews while RadioPost is open.",
        ],
    )
    f.save(OUT / "callsigns.svg")


# ------------------------------------------------------------------------------------------------
# A gateway station
# ------------------------------------------------------------------------------------------------


def fig_gateway() -> None:
    f = Plate(1000, 410, "A gateway station")
    for i, (call, place) in enumerate([("N0CALL", "home station"), ("KK4ODA", "mobile")]):
        f.box(
            24,
            44 + i * 128,
            196,
            100,
            "client",
            tag="A user calls",
            title=call,
            rows=[f"RadioPost, {place}"],
            icon="person",
        )
    f.computer(296, 20, 408, 252, "The gateway's computer")
    f.box(
        316,
        58,
        172,
        190,
        "gateway",
        tag="RadioPost Gateway",
        title="Answers calls",
        rows=["checks each caller,", "passes the session", "to its post office"],
        icon="antenna",
    )
    f.box(
        512,
        58,
        172,
        190,
        "neutral",
        tag="Aether HF",
        title="The modem",
        rows=["keys the radio,", "checks the band", "rules, sends the", "Morse ID"],
    )
    f.link([(490, 153), (510, 153)], "local", head=False)
    f.box(
        780,
        44,
        196,
        100,
        "po",
        tag="Post office",
        title="radiopost.org",
        rows=["the caller's own"],
        icon="envelope",
    )
    f.box(
        780,
        172,
        196,
        100,
        "po",
        tag="Post office",
        title="hampost.org",
        rows=["or any other one"],
        icon="envelope",
    )
    f.link([(224, 94), (292, 124)], "radio", both=True)
    f.link([(224, 222), (292, 190)], "radio", both=True)
    f.text(258, 166, "Radio", size=12.5, weight=600, color=LINKS["radio"][0], anchor="middle")
    f.link([(708, 124), (776, 94)], "internet", both=True)
    f.link([(708, 190), (776, 222)], "internet", both=True)
    f.text(742, 166, "Internet", size=12.5, weight=600, color=LINKS["internet"][0], anchor="middle")

    f.panel(24, 296, 952, 78, "What it takes")
    f.note(
        38,
        338,
        [
            "A licensed operator, a radio left on one channel, Aether HF, RadioPost Gateway,"
            " and the RadioPost client enrolled as you.",
            "An Internet connection: the gateway only connects out, so a home router needs"
            " no change.",
        ],
    )
    f.legend(400, [*LEGEND[:2], LOCAL])
    f.save(OUT / "gateway.svg")


# ------------------------------------------------------------------------------------------------
# When the Internet is down
# ------------------------------------------------------------------------------------------------


def fig_cutoff() -> None:
    f = Plate(1000, 450, "When the Internet is down")
    red = ROLES["warn"][1]
    f.text(24, 30, "Before: your mail is copied ahead", size=15, weight=700, color=INK)
    f.box(
        24,
        46,
        220,
        104,
        "client",
        tag="You, once, in Setup",
        title="Usual gateways",
        rows=["KK4ODA-1 ticked,", "then Save"],
        icon="person",
    )
    f.box(
        390,
        46,
        220,
        104,
        "gateway",
        tag="Your usual gateway",
        title="KK4ODA-1",
        rows=["holds copies of the", "mail waiting for you"],
        icon="antenna",
    )
    f.box(
        756,
        46,
        220,
        104,
        "po",
        tag="Your post office",
        title="radiopost.org",
        rows=["copies each new message", "to your usual gateways"],
        icon="envelope",
    )
    f.link([(752, 98), (614, 98)], "internet", label="Copies ahead", dy=-10)
    f.link(
        [(134, 152), (134, 172), (866, 172), (866, 154)],
        "internet",
        label="Saved at your post office",
        at=0.5,
        dy=-7,
    )

    f.text(24, 212, "During: the gateway answers on its own", size=15, weight=700, color=INK)
    f.box(
        24,
        226,
        220,
        104,
        "client",
        tag="You call",
        title="Mail both ways",
        rows=["What you send shows", "queued at KK4ODA-1"],
        icon="person",
    )
    f.box(
        390,
        226,
        220,
        104,
        "gateway",
        tag="The gateway",
        title="Serves the copies",
        rows=["and keeps what you", "send, to report later"],
        icon="antenna",
    )
    f.box(
        756,
        226,
        220,
        104,
        "po",
        tag="Your post office",
        title="Out of reach",
        rows=["the Internet is down", "at the gateway"],
        icon="envelope",
    )
    f.link([(248, 278), (386, 278)], "radio", both=True, label="Radio")
    f.link([(614, 278), (666, 278)], "fail", head=False)
    f.cross(683, 278, red)
    f.link([(700, 278), (752, 278)], "fail", head=False)

    f.panel(24, 352, 952, 74, "After")
    f.note(
        38,
        396,
        [
            "When the Internet is back, the gateway reports each session to your post office, and"
            " your outbox marks the mail sent.",
            "A partner gateway that still has the Internet can carry it sooner, by radio.",
        ],
    )
    f.legend(444, [*LEGEND[:2], ("fail", "Out of reach")])
    f.save(OUT / "cutoff.svg")


FIGURES = [
    fig_overview,
    fig_station,
    fig_joining,
    fig_journey,
    fig_direct,
    fig_callsigns,
    fig_gateway,
    fig_cutoff,
]


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    for draw in FIGURES:
        draw()
    print(f"{len(FIGURES)} figures in {OUT}")


if __name__ == "__main__":
    main()
