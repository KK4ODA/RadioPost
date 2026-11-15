# SPDX-License-Identifier: MIT OR Apache-2.0
"""The RadioPost user's manual: its contents, its checks, and its printable forms.

    python tools/manual/build.py toc            # rewrite the contents from the headings
    python tools/manual/build.py check          # the manual agrees with itself (CI)
    python tools/manual/build.py html [OUT]     # one self-contained web page, to read or print
    python tools/manual/build.py pdf [OUT]      # the PDF, through Google Chrome or Chromium

The manual is docs/manual/manual.md, written in the plain Markdown GitHub shows. `check` fails
when the contents are stale, a picture is missing or has no description, a figure's number is out
of order, a link inside the manual leads nowhere, a figure or screenshot is not used, or the
release the manual describes is not the repository's version (tools/release.py).

`html` and `pdf` turn it into one page with a cover, the contents and page numbers, the pictures
inside it, ready for Letter paper (and A4). They write build/manual/ by default. `pdf` looks for
Chrome or Chromium on the PATH, in the usual places, or where $CHROME says. Its pictures are drawn
in Inter when that font is installed. Standard library only, like the other tools here.
"""

from __future__ import annotations

import base64
import html
import os
import re
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MANUAL = ROOT / "docs" / "manual" / "manual.md"
BUILD = ROOT / "build" / "manual"
sys.path.insert(0, str(ROOT / "tools"))

import release  # ruff: ignore[module-import-not-at-top-of-file] - after its path, above

TOC_START = "<!-- toc -->"
TOC_END = "<!-- /toc -->"
HEADING = re.compile(r"^(#{1,6}) (.+?)\s*$")
IMAGE = re.compile(r"!\[([^\]]*)\]\(([^)\s]+)\)")
CAPTION = re.compile(r"^\*Figure (\d+)\. ")
LINK = re.compile(r"(?<!!)\[([^\]]+)\]\(([^)\s]+)\)")
DESCRIBES = re.compile(r"^\| \*\*Describes\*\* \| RadioPost ([^ |]+) \|$", re.MULTILINE)
FENCE = "```"


# ── headings and the contents ────────────────────────────────────────────────────────────────


def slug(text: str) -> str:
    """The anchor GitHub gives a heading: lowercase, punctuation out, spaces to hyphens."""
    plain = re.sub(r"[`*_]|\[([^\]]*)\]\([^)]*\)", r"\1", text).strip().lower()
    plain = "".join(c for c in plain if c.isalnum() or c in " -_")
    return plain.replace(" ", "-")


@dataclass
class Heading:
    level: int
    text: str
    anchor: str


def headings(lines: list[str]) -> list[Heading]:
    """Every heading outside code, with GitHub's anchor (a repeat gets -1, -2…)."""
    found: list[Heading] = []
    seen: dict[str, int] = {}
    fenced = False
    for line in lines:
        if line.startswith(FENCE):
            fenced = not fenced
            continue
        m = None if fenced else HEADING.match(line)
        if not m:
            continue
        base = slug(m.group(2))
        n = seen.get(base, 0)
        seen[base] = n + 1
        found.append(Heading(len(m.group(1)), m.group(2), f"{base}-{n}" if n else base))
    return found


def contents(lines: list[str]) -> list[str]:
    """The contents: the parts, and the chapters under them; not the title or the contents."""
    rows = []
    for h in headings(lines)[1:]:
        if h.text == "Contents":
            continue
        if h.level == 1:
            rows.append(f"\n**[{h.text}](#{h.anchor})**\n")
        elif h.level == 2:
            rows.append(f"- [{h.text}](#{h.anchor})")
    return ["", *"\n".join(rows).strip("\n").split("\n"), ""]


def with_contents(text: str) -> str:
    lines = text.split("\n")
    start, end = lines.index(TOC_START), lines.index(TOC_END)
    return "\n".join([*lines[: start + 1], *contents(lines), *lines[end:]])


# ── the checks ───────────────────────────────────────────────────────────────────────────────


def problems_in(text: str) -> list[str]:
    lines = text.split("\n")
    found: list[str] = []
    if with_contents(text) != text:
        found.append("the contents are stale: run `python tools/manual/build.py toc`")
    anchors = {h.anchor for h in headings(lines)}
    for _label, target in LINK.findall(text):
        if target.startswith("#") and target[1:] not in anchors:
            found.append(f"a link to {target}, which is no heading here")
    used = set()
    expected = 1
    for i, line in enumerate(lines):
        m = IMAGE.fullmatch(line.strip())
        if not m:
            continue
        alt, src = m.groups()
        used.add(src)
        if not alt.strip():
            found.append(f"line {i + 1}: {src} has no description")
        if not (MANUAL.parent / src).exists():
            found.append(f"line {i + 1}: {src} does not exist")
        nxt = next((x for x in lines[i + 1 :] if x.strip()), "")
        cap = CAPTION.match(nxt)
        if not cap:
            found.append(f"line {i + 1}: {src} has no caption *Figure N. …* under it")
        elif int(cap.group(1)) != expected:
            found.append(f"line {i + 1}: {src} is captioned Figure {cap.group(1)}, not {expected}")
        expected += 1
    for folder in ("fig", "screens"):
        for path in sorted((MANUAL.parent / folder).glob("*")):
            if f"{folder}/{path.name}" not in used:
                found.append(f"{folder}/{path.name} is in no figure: use it or delete it")
    m = DESCRIBES.search(text)
    version = release_version()
    if not m:
        found.append("the header has no `| **Describes** | RadioPost X |` row")
    elif m.group(1) != version:
        found.append(f"it describes {m.group(1)}, the repository is {version}")
    return found


def release_version() -> str:
    versions = {v for _, v in release.sites() if v}
    return next(iter(versions)) if len(versions) == 1 else "(the version sites disagree)"


# ── Markdown, the manual's part of it, to HTML ───────────────────────────────────────────────


def inline(text: str) -> str:
    """Code, pictures, links, bold and italics, with everything else escaped."""
    kept: list[str] = []

    def keep(markup: str) -> str:
        kept.append(markup)
        return f"\x00{len(kept) - 1}\x00"

    text = re.sub(r"`([^`]+)`", lambda m: keep(f"<code>{html.escape(m.group(1))}</code>"), text)
    text = IMAGE.sub(
        lambda m: keep(f'<img src="{html.escape(m.group(2))}" alt="{html.escape(m.group(1))}">'),
        text,
    )

    def link(m: re.Match[str]) -> str:
        return keep(f'<a href="{html.escape(m.group(2))}">') + m.group(1) + keep("</a>")

    text = LINK.sub(link, text)
    text = html.escape(text, quote=False)
    text = re.sub(r"\*\*(.+?)\*\*", r"<strong>\1</strong>", text)
    text = re.sub(r"(?<![\w*])\*(?!\s)(.+?)(?<!\s)\*(?![\w*])", r"<em>\1</em>", text)
    return re.sub(r"\x00(\d+)\x00", lambda m: kept[int(m.group(1))], text)


LIST_ITEM = re.compile(r"^( *)([-*]|\d+\.) +(.*)$")


@dataclass
class Page:
    """What the converter collects on the way: the headings' anchors, in order."""

    anchors: list[str] = field(default_factory=list)
    seen: dict[str, int] = field(default_factory=dict)

    def anchor(self, text: str) -> str:
        base = slug(text)
        n = self.seen.get(base, 0)
        self.seen[base] = n + 1
        return f"{base}-{n}" if n else base


def blocks(lines: list[str], page: Page) -> str:
    out: list[str] = []
    i = 0
    while i < len(lines):
        line = lines[i]
        stripped = line.strip()
        if not stripped or stripped.startswith("<!--"):
            i += 1
        elif line.startswith(FENCE):
            end = lines.index(FENCE, i + 1) if FENCE in lines[i + 1 :] else len(lines)
            out.append(f"<pre><code>{html.escape(chr(10).join(lines[i + 1 : end]))}</code></pre>")
            i = end + 1
        elif m := HEADING.match(line):
            level, text = len(m.group(1)), m.group(2)
            out.append(f'<h{level} id="{page.anchor(text)}">{inline(text)}</h{level}>')
            i += 1
        elif stripped == "---":
            out.append("<hr>")
            i += 1
        elif (
            line.startswith("|") and i + 1 < len(lines) and re.match(r"^\|[-| :]+\|$", lines[i + 1])
        ):
            i = table(lines, i, out)
        elif line.startswith(">"):
            j = i
            while j < len(lines) and lines[j].startswith(">"):
                j += 1
            inner = [re.sub(r"^> ?", "", x) for x in lines[i:j]]
            out.append(f'<aside class="note">{blocks(inner, page)}</aside>')
            i = j
        elif LIST_ITEM.match(line):
            i = listing(lines, i, out, page)
        else:
            i = paragraph(lines, i, out)
    return "\n".join(out)


def table(lines: list[str], i: int, out: list[str]) -> int:
    def cells(row: str) -> list[str]:
        return [c.strip() for c in row.strip().strip("|").split("|")]

    head = cells(lines[i])
    j = i + 2
    rows = []
    while j < len(lines) and lines[j].startswith("|"):
        rows.append(cells(lines[j]))
        j += 1
    th = "".join(f"<th>{inline(c)}</th>" for c in head)
    body = "".join("<tr>" + "".join(f"<td>{inline(c)}</td>" for c in r) + "</tr>" for r in rows)
    cls = ' class="plain"' if not any(head) else ""
    out.append(f"<table{cls}><thead><tr>{th}</tr></thead><tbody>{body}</tbody></table>")
    return j


def listing(lines: list[str], i: int, out: list[str], page: Page) -> int:
    first = LIST_ITEM.match(lines[i])
    assert first
    indent = len(first.group(1))
    ordered = first.group(2)[0].isdigit()
    items: list[list[str]] = []
    j = i
    while j < len(lines):
        m = LIST_ITEM.match(lines[j])
        if m and len(m.group(1)) == indent and m.group(2)[0].isdigit() == ordered:
            width = indent + len(m.group(2)) + 1
            item = [m.group(3)]
            j += 1
            while j < len(lines):
                nxt = lines[j]
                if nxt.strip() and len(nxt) - len(nxt.lstrip()) >= width:
                    item.append(nxt[width:])
                elif (
                    not nxt.strip() and j + 1 < len(lines) and lines[j + 1].startswith(" " * width)
                ):
                    item.append("")
                else:
                    break
                j += 1
            items.append(item)
        else:
            break
    tag = "ol" if ordered else "ul"
    start = int(first.group(2)[:-1]) if ordered else 1
    attr = f' start="{start}"' if ordered and start != 1 else ""
    rendered = []
    for item in items:
        inner = blocks(item, page)
        if inner.startswith("<p>") and inner.count("<p>") == 1:
            inner = inner[3:].replace("</p>", "", 1)
        rendered.append(f"<li>{inner}</li>")
    out.append(f"<{tag}{attr}>{''.join(rendered)}</{tag}>")
    return j


def paragraph(lines: list[str], i: int, out: list[str]) -> int:
    j = i
    while j < len(lines) and lines[j].strip() and not starts_block(lines[j]):
        j += 1
    j = max(j, i + 1)
    text = " ".join(x.strip() for x in lines[i:j])
    picture = IMAGE.fullmatch(text)
    if picture:
        out.append(
            f'<figure><img src="{html.escape(picture.group(2))}" '
            f'alt="{html.escape(picture.group(1))}"></figure>'
        )
    elif CAPTION.match(text) and out and out[-1].startswith("<figure>"):
        out[-1] = out[-1].replace(
            "</figure>", f"<figcaption>{inline(text.strip('*'))}</figcaption></figure>"
        )
    else:
        out.append(f"<p>{inline(text)}</p>")
    return j


def starts_block(line: str) -> bool:
    return bool(
        HEADING.match(line)
        or LIST_ITEM.match(line)
        or line.startswith(("|", ">", FENCE))
        or line.strip() == "---"
    )


# ── the page ─────────────────────────────────────────────────────────────────────────────────

STYLE = """
@page { size: letter; margin: 0.8in 0.75in 0.85in;
  @bottom-center { content: counter(page); font: 9pt Inter, sans-serif; color: #64748b; }
  @top-right { content: "RadioPost user's manual"; font: 8.5pt Inter, sans-serif;
               color: #94a3b8; } }
@page :first { @bottom-center { content: none; } @top-right { content: none; } }
:root { --ink: #1e293b; --muted: #475569; --hair: #cbd5e1; --accent: #1d4ed8;
        --warm: #c2410c; }
html { font: 10.5pt/1.5 Inter, "Segoe UI", "Helvetica Neue", Arial, sans-serif; color: var(--ink); }
body { margin: 0 auto; max-width: 46rem; padding: 1.5rem; }
h1, h2, h3 { line-height: 1.25; break-after: avoid; }
h1 { font-size: 22pt; margin: 0 0 1rem; color: var(--accent); }
h2 { font-size: 15pt; margin: 2rem 0 0.6rem; padding-top: 0.4rem;
     border-top: 1px solid var(--hair); }
h3 { font-size: 12pt; margin: 1.4rem 0 0.4rem; }
a { color: var(--accent); text-decoration: none; }
code { font: 9pt "JetBrains Mono", Consolas, "DejaVu Sans Mono", monospace; background: #f1f5f9;
       padding: 0 0.2em; border-radius: 3px; }
pre { background: #f1f5f9; padding: 0.6rem 0.8rem; border-radius: 6px; white-space: pre-wrap; }
pre code { background: none; padding: 0; }
table { border-collapse: collapse; width: 100%; margin: 0.8rem 0; font-size: 9.5pt; }
th, td { border-bottom: 1px solid var(--hair); padding: 0.3rem 0.5rem; text-align: left;
         vertical-align: top; }
th { background: #f8fafc; }
tr { break-inside: avoid; }
table.plain thead { display: none; }
table.plain td:first-child { width: 9rem; }
figure { margin: 1rem 0 1.2rem; break-inside: avoid; text-align: center; }
figure img { box-sizing: border-box; max-width: 100%; max-height: 7.6in;
             border: 1px solid var(--hair); border-radius: 6px; }
figcaption { font-size: 9pt; color: var(--muted); margin-top: 0.35rem; }
aside.note { border-left: 4px solid var(--warm); background: #fff7ed; padding: 0.2rem 0.9rem;
             margin: 1rem 0; break-inside: avoid; border-radius: 0 6px 6px 0; }
li { margin: 0.15rem 0; }
.cover { height: 9in; display: flex; flex-direction: column; justify-content: center;
         break-after: page; }
.cover h1 { font-size: 34pt; color: var(--ink); margin-bottom: 0.3rem; }
.cover .sub { font-size: 14pt; color: var(--muted); margin: 0 0 2.5rem; }
.cover img { width: 100%; border: none; }
.cover dl { display: grid; grid-template-columns: max-content 1fr; gap: 0.2rem 1rem;
            margin-top: 2.5rem; color: var(--muted); }
.cover dt { font-weight: 700; color: var(--ink); }
nav.toc ul { list-style: none; padding-left: 0.8rem; margin: 0.1rem 0 0.4rem; }
nav.toc li { margin: 0; }
nav.toc p { margin: 0.5rem 0 0.1rem; break-after: avoid; }
nav.toc .columns { columns: 2; column-gap: 2rem; font-size: 9.5pt; }
nav.toc .columns p:first-child { margin-top: 0; }
.part { break-before: page; font-size: 26pt; margin-top: 0; }
@media print { body { padding: 0; max-width: none; } h2 { border-top: none; } }
@media screen { html { background: #f1f5f9; } body { background: #fff; } }
"""


def embed(src: str) -> str:
    """A picture of the manual as a data address, so the page carries it with it."""
    path = (MANUAL.parent / src).resolve()
    kind = {".svg": "image/svg+xml", ".png": "image/png"}.get(
        path.suffix, "application/octet-stream"
    )
    return f"data:{kind};base64,{base64.b64encode(path.read_bytes()).decode()}"


def commit() -> str:
    try:
        return subprocess.run(
            ["git", "-C", str(ROOT), "rev-parse", "--short", "HEAD"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return ""


def page_html(text: str) -> str:
    lines = text.split("\n")
    header = dict(re.findall(r"^\| \*\*(\w+)\*\* \| ([^|]+?) \|$", text, re.MULTILINE))
    start, end = lines.index(TOC_START), lines.index(TOC_END)
    toc_html = blocks(lines[start + 1 : end], Page())
    # the words between the header and the contents go before the contents; the title and the
    # header become the cover
    table_end = next(i for i, x in enumerate(lines) if i and lines[i - 1].startswith("|") and not x)
    intro = blocks(lines[table_end : lines.index("## Contents")], Page())
    page = Page()
    for h in headings(lines[: end + 1]):
        page.anchor(h.text)
    body = blocks(lines[end + 1 :], page)
    body = body.replace("<h1 id=", '<h1 class="part" id=')
    body = re.sub(r'src="((?:fig|screens)/[^"]+)"', lambda m: f'src="{embed(m.group(1))}"', body)
    built = datetime.now(UTC).strftime("%Y-%m-%d")
    made = f"{built}, commit {commit()}" if commit() else built
    cover = f"""
<section class="cover">
  <h1>RadioPost user's manual</h1>
  <p class="sub">The RadioPost client and RadioPost Gateway: email by amateur radio.</p>
  <img src="{embed("fig/overview.svg")}" alt="How RadioPost carries your email">
  <dl>
    <dt>Manual version</dt><dd>{html.escape(header.get("Version", ""))}, \
updated {html.escape(header.get("Updated", ""))}</dd>
    <dt>Describes</dt><dd>{html.escape(header.get("Describes", ""))}</dd>
    <dt>Built</dt><dd>{html.escape(made)}</dd>
    <dt>Licence</dt><dd>CC BY 4.0. github.com/KK4ODA/RadioPost</dd>
  </dl>
</section>"""
    return (
        '<!doctype html>\n<html lang="en"><head><meta charset="utf-8">'
        '<meta name="viewport" content="width=device-width, initial-scale=1">'
        f"<title>RadioPost user's manual</title><style>{STYLE}</style></head><body>"
        f'{cover}<nav class="toc">{intro}<h1>Contents</h1><div class="columns">{toc_html}</div>'
        f"</nav>{body}</body></html>\n"
    )


def chrome() -> str:
    named = os.environ.get("CHROME")
    if named:
        return named
    for name in (
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "chrome",
        "msedge",
    ):
        found = shutil.which(name)
        if found:
            return found
    for path in (
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
    ):
        if Path(path).exists():
            return path
    raise SystemExit("no Chrome or Chromium found: set $CHROME, or use `html` and print it")


def write_pdf(page: str, out: Path) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        source = Path(tmp) / "manual.html"
        source.write_text(page, encoding="utf-8")
        subprocess.run(
            [
                chrome(),
                "--headless=new",
                "--disable-gpu",
                "--no-sandbox",
                "--no-pdf-header-footer",
                "--print-to-pdf-no-header",
                "--virtual-time-budget=20000",
                f"--print-to-pdf={out}",
                source.as_uri(),
            ],
            check=True,
            capture_output=True,
        )


def default_out(suffix: str) -> Path:
    return BUILD / f"RadioPost-Manual-{release_version()}.{suffix}"


def main() -> int:
    args = sys.argv[1:]
    text = MANUAL.read_text(encoding="utf-8")
    command = args[0] if args else ""
    if command == "toc":
        MANUAL.write_text(with_contents(text), encoding="utf-8")
        print("contents rewritten")
        return 0
    if command == "check":
        found = problems_in(text)
        for line in found:
            print(f"docs/manual/manual.md: {line}", file=sys.stderr)
        if not found:
            print("the manual's contents, pictures, links and version agree")
        return 1 if found else 0
    if command in {"html", "pdf"}:
        found = problems_in(text)
        if found:
            print("\n".join(found), file=sys.stderr)
            return 1
        out = Path(args[1]) if len(args) > 1 else default_out(command)
        out.parent.mkdir(parents=True, exist_ok=True)
        page = page_html(text)
        if command == "html":
            out.write_text(page, encoding="utf-8")
        else:
            write_pdf(page, out.resolve())
        print(out)
        return 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
