# SPDX-License-Identifier: MIT OR Apache-2.0
"""The drawing kit RadioPost's figures share.

Every figure uses one style: a role colour per kind of station (radio user, gateway, post
office, ordinary email) and a line style per kind of link (over the air, the post office API,
ordinary email, a local connection), so a reader learns them once. Standard library only.
"""

from __future__ import annotations

from collections.abc import Iterable, Sequence
from itertools import pairwise
from pathlib import Path
from xml.sax.saxutils import escape

FONT = "Inter, 'Segoe UI', 'Helvetica Neue', Helvetica, Arial, sans-serif"
MONO = "'JetBrains Mono', 'Cascadia Mono', Consolas, 'DejaVu Sans Mono', Menlo, monospace"

INK = "#1e293b"
MUTED = "#475569"
FAINT = "#94a3b8"
HAIR = "#cbd5e1"
PAPER = "#ffffff"

# fill, stroke
ROLES = {
    "client": ("#eff6ff", "#1d4ed8"),
    "gateway": ("#fff7ed", "#c2410c"),
    "po": ("#ecfdf5", "#047857"),
    "mail": ("#f5f3ff", "#6d28d9"),
    "neutral": ("#f8fafc", "#475569"),
    "warn": ("#fef2f2", "#b91c1c"),
    "paper": ("#ffffff", "#94a3b8"),
}

# colour, dash, width
LINKS = {
    "radio": ("#c2410c", "8 5", 2.2),
    "internet": ("#334155", None, 2.0),
    "smtp": ("#6d28d9", None, 2.0),
    "local": ("#64748b", "2 4", 1.8),
    "fail": ("#b91c1c", "5 4", 2.0),
}

Point = tuple[float, float]


class Fig:
    """An SVG drawing with the shared style."""

    def __init__(self, width: int, height: int, title: str, subtitle: str = "") -> None:
        self.w = width
        self.h = height
        self.body: list[str] = []
        self.title = title
        self.subtitle = subtitle

    # -- primitives -----------------------------------------------------------------------------

    def add(self, s: str) -> None:
        self.body.append(s)

    def text(
        self,
        x: float,
        y: float,
        s: str,
        *,
        size: float = 13.5,
        weight: int = 400,
        color: str = INK,
        anchor: str = "start",
        mono: bool = False,
        halo: bool = False,
        italic: bool = False,
    ) -> None:
        family = MONO if mono else FONT
        extra = ""
        if halo:
            extra += (
                f' paint-order="stroke" stroke="{PAPER}" stroke-width="5" stroke-linejoin="round"'
            )
        if italic:
            extra += ' font-style="italic"'
        self.add(
            f'<text xml:space="preserve" x="{x:.1f}" y="{y:.1f}" font-family="{family}" '
            f'font-size="{size}" '
            f'font-weight="{weight}" fill="{color}" text-anchor="{anchor}"{extra}>'
            f"{escape(s)}</text>"
        )

    def lines(
        self,
        x: float,
        y: float,
        rows: Iterable[str],
        size: float = 13.5,
        gap: float = 1.45,
        **kw: object,
    ) -> float:
        """Draw rows of text from baseline ``y``; return the baseline after the last."""
        for row in rows:
            self.text(x, y, row, size=size, **kw)  # type: ignore[arg-type]
            y += size * gap
        return y

    def rect(  # ruff: ignore[too-many-positional-arguments] - a rectangle and its two colours, as SVG orders them
        self,
        x: float,
        y: float,
        w: float,
        h: float,
        fill: str,
        stroke: str,
        rx: float = 10,
        sw: float = 1.5,
        dash: str | None = None,
        opacity: float = 1.0,
    ) -> None:
        d = f' stroke-dasharray="{dash}"' if dash else ""
        o = f' fill-opacity="{opacity}"' if opacity < 1 else ""
        self.add(
            f'<rect x="{x:.1f}" y="{y:.1f}" width="{w:.1f}" height="{h:.1f}" rx="{rx}" '
            f'fill="{fill}"{o} stroke="{stroke}" stroke-width="{sw}"{d}/>'
        )

    def box(
        self,
        x: float,
        y: float,
        w: float,
        h: float,
        role: str,
        *,
        tag: str | None = None,
        title: str | None = None,
        rows: Sequence[str] = (),
        mono_rows: Sequence[str] = (),
        size: float = 13.5,
        title_size: float = 15.5,
        icon: str | None = None,
        center: bool = False,
    ) -> None:
        fill, stroke = ROLES[role]
        self.rect(x, y, w, h, fill, stroke)
        tx = x + w / 2 if center else x + 14
        anchor = "middle" if center else "start"
        cy = y + 14
        if tag:
            self.text(tx, cy + 11, tag, size=11.5, weight=650, color=stroke, anchor=anchor)
            cy += 19
        if title:
            self.text(tx, cy + title_size, title, size=title_size, weight=700, anchor=anchor)
            cy += title_size + 9
        cy += size
        cy = self.lines(tx, cy, rows, size=size, color=MUTED, anchor=anchor)
        if mono_rows:
            self.lines(tx, cy + 2, mono_rows, size=size - 1.5, color=INK, anchor=anchor, mono=True)
        if icon:
            self.icon(icon, x + w - 30, y + 12, stroke)

    def panel(self, x: float, y: float, w: float, h: float, heading: str) -> None:
        """A plain framed area with a heading, for examples and notes."""
        self.rect(x, y, w, h, "#f8fafc", HAIR, rx=8, sw=1.2)
        self.text(x + 14, y + 24, heading, size=13, weight=700, color=MUTED)

    # -- links ----------------------------------------------------------------------------------

    def link(
        self,
        pts: Sequence[Point],
        kind: str = "internet",
        *,
        label: Sequence[str] | str | None = None,
        at: float = 0.5,
        dx: float = 0,
        dy: float = -8,
        both: bool = False,
        head: bool = True,
        label_color: str | None = None,
        anchor: str = "middle",
        size: float = 12.5,
    ) -> None:
        color, dash, width = LINKS[kind]
        d = "M " + " L ".join(f"{px:.1f} {py:.1f}" for px, py in pts)
        dash_attr = f' stroke-dasharray="{dash}"' if dash else ""
        mid = f"ah-{kind}"
        end = f' marker-end="url(#{mid})"' if head else ""
        start = f' marker-start="url(#{mid})"' if both else ""
        self.add(
            f'<path d="{d}" fill="none" stroke="{color}" stroke-width="{width}"'
            f'{dash_attr} stroke-linecap="round" stroke-linejoin="round"{start}{end}/>'
        )
        if label:
            rows = [label] if isinstance(label, str) else list(label)
            lx, ly = point_along(pts, at)
            lx += dx
            ly += dy - (len(rows) - 1) * size * 1.3
            for row in rows:
                self.text(
                    lx,
                    ly,
                    row,
                    size=size,
                    weight=600,
                    color=label_color or color,
                    anchor=anchor,
                    halo=True,
                )
                ly += size * 1.3

    def badge(self, x: float, y: float, s: str, role: str = "neutral", size: float = 11.5) -> None:
        """A small rounded label, centred on ``x``."""
        fill, stroke = ROLES[role]
        w = len(s) * size * 0.58 + 16
        self.rect(x - w / 2, y - size - 4, w, size + 9, fill, stroke, rx=(size + 9) / 2, sw=1.1)
        self.text(x, y, s, size=size, weight=650, color=stroke, anchor="middle")

    def number(self, x: float, y: float, n: str, color: str = INK) -> None:
        """A numbered step marker."""
        self.add(
            f'<circle cx="{x:.1f}" cy="{y:.1f}" r="11" fill="{color}" stroke="{PAPER}" '
            'stroke-width="2"/>'
        )
        self.text(x, y + 4.5, n, size=12.5, weight=700, color=PAPER, anchor="middle")

    def cross(self, x: float, y: float, color: str = "#b91c1c") -> None:
        self.add(
            f'<circle cx="{x:.1f}" cy="{y:.1f}" r="12" fill="{PAPER}" stroke="{color}" '
            'stroke-width="2"/>'
            f'<path d="M {x - 5:.1f} {y - 5:.1f} L {x + 5:.1f} {y + 5:.1f} M {x + 5:.1f} '
            f'{y - 5:.1f} L {x - 5:.1f} {y + 5:.1f}" stroke="{color}" stroke-width="2.4" '
            'stroke-linecap="round"/>'
        )

    # -- icons ----------------------------------------------------------------------------------

    def icon(self, kind: str, x: float, y: float, color: str) -> None:
        """An 18-pixel line icon with its top-left corner at (x, y)."""
        s = f'fill="none" stroke="{color}" stroke-width="1.6" stroke-linecap="round"'
        if kind == "envelope":
            self.add(
                f'<g {s} stroke-linejoin="round"><rect x="{x}" y="{y + 3}" width="18" '
                f'height="13" rx="2"/><path d="M {x} {y + 4} L {x + 9} {y + 11} L {x + 18} '
                f'{y + 4}"/></g>'
            )
        elif kind == "antenna":
            cx = x + 9
            self.add(
                f'<g {s}><path d="M {cx} {y + 6} L {cx} {y + 18} M {cx - 4} {y + 18} L {cx + 4}'
                f' {y + 18}"/><circle cx="{cx}" cy="{y + 5}" r="1.6" fill="{color}"/>'
                f'<path d="M {cx - 4} {y + 1.5} Q {cx - 6.5} {y + 5} {cx - 4} {y + 8.5}"/>'
                f'<path d="M {cx + 4} {y + 1.5} Q {cx + 6.5} {y + 5} {cx + 4} {y + 8.5}"/>'
                f'<path d="M {cx - 7.5} {y - 1} Q {cx - 11} {y + 5} {cx - 7.5} {y + 11}"/>'
                f'<path d="M {cx + 7.5} {y - 1} Q {cx + 11} {y + 5} {cx + 7.5} {y + 11}"/></g>'
            )
        elif kind == "person":
            self.add(
                f'<g {s}><circle cx="{x + 9}" cy="{y + 5.5}" r="4"/>'
                f'<path d="M {x + 1.5} {y + 18} Q {x + 2} {y + 11} {x + 9} {y + 11} Q {x + 16}'
                f' {y + 11} {x + 16.5} {y + 18}"/></g>'
            )
        elif kind == "server":
            self.add(
                f'<g {s} stroke-linejoin="round"><rect x="{x}" y="{y + 1}" width="18" '
                f'height="7" rx="1.5"/><rect x="{x}" y="{y + 10}" width="18" height="7" '
                f'rx="1.5"/><circle cx="{x + 4}" cy="{y + 4.5}" r="0.9" fill="{color}"/>'
                f'<circle cx="{x + 4}" cy="{y + 13.5}" r="0.9" fill="{color}"/></g>'
            )
        elif kind == "globe":
            self.add(
                f'<g {s}><circle cx="{x + 9}" cy="{y + 9}" r="8"/><ellipse cx="{x + 9}" '
                f'cy="{y + 9}" rx="3.5" ry="8"/><path d="M {x + 1} {y + 9} L {x + 17} '
                f'{y + 9}"/></g>'
            )
        elif kind == "key":
            self.add(
                f'<g {s}><circle cx="{x + 5}" cy="{y + 9}" r="4"/><path d="M {x + 9} {y + 9} '
                f"L {x + 18} {y + 9} M {x + 15} {y + 9} L {x + 15} {y + 13} M {x + 18} {y + 9}"
                f' L {x + 18} {y + 12}"/></g>'
            )
        elif kind == "doc":
            self.add(
                f'<g {s} stroke-linejoin="round"><path d="M {x + 2} {y} L {x + 12} {y} L '
                f'{x + 16} {y + 4} L {x + 16} {y + 18} L {x + 2} {y + 18} Z"/><path d="M '
                f"{x + 5} {y + 8} L {x + 13} {y + 8} M {x + 5} {y + 11.5} L {x + 13} {y + 11.5}"
                f' M {x + 5} {y + 15} L {x + 10} {y + 15}"/></g>'
            )

    # -- legend and output ----------------------------------------------------------------------

    def legend(self, y: float, kinds: Sequence[tuple[str, str]], x: float = 24) -> None:
        for kind, label in kinds:
            self.link([(x, y - 4), (x + 38, y - 4)], kind=kind, head=False)
            self.text(x + 46, y, label, size=12.5, color=MUTED)
            x += 46 + len(label) * 6.9 + 34

    def svg(self) -> str:
        defs = []
        for kind, (color, _dash, _w) in LINKS.items():
            defs.append(
                f'<marker id="ah-{kind}" viewBox="0 0 10 10" refX="8.5" refY="5" '
                'markerWidth="7" markerHeight="7" orient="auto-start-reverse">'
                f'<path d="M 0 0 L 10 5 L 0 10 z" fill="{color}"/></marker>'
            )
        head = [
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {self.w} {self.h}" '
            f'width="{self.w}" height="{self.h}" role="img" aria-label="{escape(self.title)}">',
            f"<defs>{''.join(defs)}</defs>",
            f'<rect x="0" y="0" width="{self.w}" height="{self.h}" fill="{PAPER}"/>',
        ]
        head.append(
            f'<text x="24" y="34" font-family="{FONT}" font-size="18" font-weight="700" '
            f'fill="{INK}">{escape(self.title)}</text>'
        )
        if self.subtitle:
            head.append(
                f'<text x="24" y="56" font-family="{FONT}" font-size="13.5" fill="{MUTED}">'
                f"{escape(self.subtitle)}</text>"
            )
        return "\n".join([*head, *self.body, "</svg>", ""])

    def save(self, path: Path) -> None:
        path.write_text(self.svg(), encoding="utf-8")


def point_along(pts: Sequence[Point], t: float) -> Point:
    """The point a fraction ``t`` of the way along a polyline."""
    segs = [(a, b, ((b[0] - a[0]) ** 2 + (b[1] - a[1]) ** 2) ** 0.5) for a, b in pairwise(pts)]
    total = sum(s[2] for s in segs)
    goal = total * t
    for a, b, n in segs:
        if goal <= n and n > 0:
            f = goal / n
            return (a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f)
        goal -= n
    return pts[-1]


LEGEND = [
    ("radio", "Over the air (HF or VHF)"),
    ("internet", "Gateway and post office (HTTPS)"),
    ("smtp", "Ordinary email (SMTP, IMAP)"),
]
