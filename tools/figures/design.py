# SPDX-License-Identifier: MIT OR Apache-2.0
"""Draw the figures of the detailed design and the specifications.

Run from anywhere: ``python tools/figures/design.py``. It writes docs/design/fig/d1-interfaces.svg
and spec/fig/rp3-session.svg and rp4-modem-interface.svg, in the style of the concept's figures.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from style import HAIR, INK, LINKS, MUTED, ROLES, Fig

ROOT = Path(__file__).resolve().parents[2]
DESIGN = ROOT / "docs" / "design" / "fig"
SPEC = ROOT / "spec" / "fig"

SPEC_INK = "#0f172a"


def spec_tag(f: Fig, x: float, y: float, text: str) -> None:
    """A dark label naming the specification that governs a connection, centred on (x, y)."""
    w = len(text) * 7.4 + 18
    f.rect(x - w / 2, y - 13, w, 24, SPEC_INK, SPEC_INK, rx=12, sw=1)
    f.text(x, y + 4, text, size=12.5, weight=700, color="#ffffff", anchor="middle")


# ------------------------------------------------------------------------------------------------
# D1: which specification governs which connection
# ------------------------------------------------------------------------------------------------


def fig_interfaces() -> None:
    f = Fig(
        1000,
        660,
        "Which specification governs which connection",
        "Agree on these, and any client, gateway, post office or modem adapter works with "
        "any other.",
    )
    # programs
    f.box(
        20,
        118,
        200,
        96,
        "client",
        tag="Client",
        title="RadioPost client",
        rows=["or anyone's client"],
        icon="person",
    )
    f.box(
        300,
        118,
        200,
        96,
        "gateway",
        tag="Gateway",
        title="Gateway program",
        rows=["or anyone's gateway"],
        icon="antenna",
    )
    f.box(
        590,
        118,
        200,
        96,
        "po",
        tag="Post office",
        title="RadioPost service",
        rows=["beside a mail server"],
        icon="envelope",
    )
    f.box(
        870,
        118,
        110,
        96,
        "mail",
        title="Email",
        rows=["Gmail, any", "provider"],
        icon="globe",
        size=12.5,
        title_size=14.5,
    )
    # adapters and modems
    for x in (20, 300):
        f.box(
            x, 300, 200, 50, "paper", title="Modem adapter", size=12.5, title_size=14, center=True
        )
        f.box(
            x,
            398,
            200,
            62,
            "paper",
            title="Modem",
            rows=["Aether, VARA, ARDOP, Direwolf"],
            size=12,
            title_size=14,
            center=True,
        )
    f.box(
        590,
        398,
        200,
        62,
        "gateway",
        title="Another gateway",
        rows=["forwarding"],
        size=12,
        title_size=14,
        center=True,
    )

    # the session, logically client to gateway
    f.link([(222, 150), (298, 150)], "radio", both=True)
    spec_tag(f, 260, 124, "RP-3")
    f.text(260, 176, "session", size=12, weight=600, color=LINKS["radio"][0], anchor="middle")
    # program to adapter, adapter to modem
    for x in (120, 400):
        f.link([(x, 216), (x, 298)], "local", both=True)
        spec_tag(f, x, 258, "RP-4")
        f.link([(x, 352), (x, 396)], "local", head=False)
    # over the air, and on to another gateway
    f.link([(222, 430), (298, 430)], "radio", both=True, label="the air", dy=-10, size=12)
    f.link([(502, 430), (588, 430)], "radio", both=True)
    spec_tag(f, 545, 404, "RP-7")
    # gateway to post office, post office to email
    f.link([(502, 166), (588, 166)], "internet", both=True)
    spec_tag(f, 545, 140, "RP-5")
    f.text(545, 192, "HTTPS", size=12, weight=600, color=LINKS["internet"][0], anchor="middle")
    f.link([(792, 166), (868, 166)], "smtp", both=True)
    spec_tag(f, 830, 140, "RP-6")
    f.text(830, 192, "SMTP", size=12, weight=600, color=LINKS["smtp"][0], anchor="middle")
    # client to post office over the Internet, when it has one
    f.link([(120, 116), (120, 92), (690, 92), (690, 116)], "internet", both=True)
    spec_tag(f, 400, 92, "RP-5 §6")
    f.text(
        458,
        84,
        "enrollment, settings; mail by JMAP or IMAP when online",
        size=12,
        color=MUTED,
        halo=True,
    )

    # identity and messages everywhere
    f.rect(20, 500, 960, 92, "#f8fafc", HAIR, rx=10, sw=1.2)
    spec_tag(f, 62, 528, "RP-1")
    f.text(
        92,
        533,
        "Identity: keys, credentials and signed session statements, checked by every station.",
        size=13,
        color=INK,
    )
    spec_tag(f, 62, 564, "RP-2")
    f.text(
        92,
        569,
        "Messages: one encoding and one content ID, the same at every station that holds them.",
        size=13,
        color=INK,
    )
    f.text(
        24,
        618,
        "RP-0 sets the conventions all of them share: encodings, cryptography, "
        "identifiers, error codes and how the specifications change.",
        size=12.5,
        color=MUTED,
    )
    f.legend(
        646,
        [
            ("radio", "Over the air"),
            ("internet", "Over the Internet"),
            ("smtp", "Ordinary email"),
            ("local", "On one machine"),
        ],
    )
    f.save(DESIGN / "d1-interfaces.svg")


# ------------------------------------------------------------------------------------------------
# RP-3: a session as specified
# ------------------------------------------------------------------------------------------------


def fig_session() -> None:
    f = Fig(
        1000,
        760,
        "A session as specified (RP-3)",
        "Facundo's client calls N1CALL-10. Five transmissions; the gateway asks his post office "
        "(RP-5) while the radio turns around.",
    )
    lanes = {"client": 150, "gateway": 520, "po": 860}
    f.box(
        40,
        80,
        220,
        60,
        "client",
        title="Client",
        rows=["facundo@radiopost.org"],
        center=True,
        size=12.5,
        title_size=14.5,
    )
    f.box(
        410,
        80,
        220,
        60,
        "gateway",
        title="Gateway N1CALL-10",
        rows=["online"],
        center=True,
        size=12.5,
        title_size=14.5,
    )
    f.box(
        750,
        80,
        220,
        60,
        "po",
        title="po.radiopost.org",
        rows=["RP-5 API"],
        center=True,
        size=12.5,
        title_size=14.5,
    )
    for x in lanes.values():
        f.add(
            f'<path d="M {x} 142 L {x} 716" stroke="{HAIR}" stroke-width="1.6" '
            'stroke-dasharray="4 5"/>'
        )
    c, g, p = lanes["client"], lanes["gateway"], lanes["po"]

    def step(  # ruff: ignore[too-many-positional-arguments] - one sequence-diagram row
        y: float, a: float, b: float, kind: str, rows: list[str], n: str | None
    ) -> None:
        f.link([(a, y), (b, y)], kind, label=rows, at=0.5, dy=-9, size=12.5)
        if n:
            f.number(a + (12 if b > a else -12), y, n, LINKS[kind][0])

    step(
        206,
        c,
        g,
        "radio",
        ["HELLO: signed statement, 2 offers,", "cursor 418, auto 1 kB, credential hash"],
        "1",
    )
    step(246, g, p, "internet", ["POST /sessions"], None)
    step(282, p, g, "internet", ["waiting 419–421; accept both"], None)
    step(
        360,
        g,
        c,
        "radio",
        ["WELCOME: accept both; waiting 419, 420, 421", "MSG #419 (under auto)"],
        "2",
    )
    step(
        440,
        c,
        g,
        "radio",
        ["MSG a3f9…, MSG 71c2…", "RECEIPT #419, REQUEST #420"],
        "3",
    )
    step(478, g, p, "internet", ["POST …/messages, POST …/pickups 419"], None)
    step(514, p, g, "internet", ["accepted; GET …/messages/420"], None)
    step(592, g, c, "radio", ["RECEIPT both, MSG #420, DONE"], "4")
    step(650, c, g, "radio", ["RECEIPT #420, DONE"], "5")
    f.text(c, 676, "the client hangs up", size=12, color=MUTED, anchor="middle")
    step(704, g, p, "internet", ["POST …/pickups 420, DELETE session"], None)
    f.legend(748, [("radio", "One transmission on the air (RP-3)"), ("internet", "HTTPS (RP-5)")])
    f.save(SPEC / "rp3-session.svg")


# ------------------------------------------------------------------------------------------------
# RP-4: the modem interface
# ------------------------------------------------------------------------------------------------


def fig_modem_interface() -> None:
    f = Fig(
        1000,
        560,
        "The modem interface (RP-4)",
        "A RadioPost program speaks one language, RPMI, to an adapter; the adapter speaks the "
        "modem's own.",
    )
    f.box(
        150,
        78,
        320,
        56,
        "client",
        title="A client",
        center=True,
        icon="person",
        title_size=15,
    )
    f.box(
        530,
        78,
        320,
        56,
        "gateway",
        title="A gateway",
        center=True,
        icon="antenna",
        title_size=15,
    )
    f.box(
        20,
        160,
        960,
        124,
        "neutral",
        tag="RPMI: JSON Lines over a local TCP port (8600) or a Unix socket",
        mono_rows=[
            '→ {"op":"connect","id":3,"remote":"N1CALL-10","as":"KK4ODA"}',
            '← {"op":"connected","id":3,"link":"a1","remote":"N1CALL-10","direction":"out"}',
            '→ {"op":"send","link":"a1","data":"AAEwoQ…","ref":"h1"}',
            '← {"op":"received","link":"a1","data":"AAJ0ow…"}',
        ],
        size=13,
    )
    for x in (310, 690):
        f.link([(x, 136), (x, 158)], "local", head=False)

    adapters = [
        ("Aether", ["built in, or Aether", "speaking RPMI itself"], ["control API", "(WebSocket)"]),
        ("VARA HF, FM", ["an adapter"], ["published TNC", "command, data ports"]),
        ("ARDOP", ["an adapter"], ["host command,", "data ports"]),
        ("AX.25 packet", ["an adapter"], ["AGWPE: Direwolf", "or a TNC"]),
        ("TCP with TLS", ["an adapter"], ["the Internet,", "AREDN mesh"]),
    ]
    w = (960 - 4 * 16) / 5
    for i, (name, rows, modem_rows) in enumerate(adapters):
        x = 20 + i * (w + 16)
        f.link([(x + w / 2, 286), (x + w / 2, 306)], "local", head=False)
        f.box(x, 308, w, 84, "paper", title=name, rows=rows, size=12, title_size=14)
        f.link([(x + w / 2, 394), (x + w / 2, 414)], "local", head=False)
        role = "gateway" if i < 4 else "mail"
        f.box(x, 416, w, 64, role, rows=modem_rows, size=12)
    f.rect(20, 500, 4 * w + 3 * 16, 40, "#fff7ed", ROLES["gateway"][1], rx=8, sw=1.2)
    f.text(
        20 + (4 * w + 3 * 16) / 2,
        526,
        "Radio: HF, VHF, UHF",
        size=14,
        weight=700,
        color=ROLES["gateway"][1],
        anchor="middle",
    )
    f.rect(20 + 4 * (w + 16), 500, w, 40, "#f5f3ff", ROLES["mail"][1], rx=8, sw=1.2)
    f.text(
        20 + 4 * (w + 16) + w / 2,
        526,
        "Internet",
        size=14,
        weight=700,
        color=ROLES["mail"][1],
        anchor="middle",
    )
    f.save(SPEC / "rp4-modem-interface.svg")


def main() -> None:
    DESIGN.mkdir(parents=True, exist_ok=True)
    SPEC.mkdir(parents=True, exist_ok=True)
    fig_interfaces()
    fig_session()
    fig_modem_interface()


if __name__ == "__main__":
    main()
