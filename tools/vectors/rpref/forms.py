# SPDX-License-Identifier: MIT OR Apache-2.0
"""RP-2 §8: forms. A template, its fingerprint, a form's values checked against it, and its text."""

from __future__ import annotations

import hashlib
from datetime import UTC, datetime
from typing import Any

from . import cbor

# RP-2 §8.1: a field's types, by number.
TYPES = {
    "line": 0,
    "text": 1,
    "date": 2,
    "time": 3,
    "number": 4,
    "choice": 5,
    "check": 6,
    "position": 7,
    "datetime": 8,
    "multi": 9,
    "rows": 10,
    "heading": 11,
}
# RP-2 §8.1: what a bound field takes from the message.
BOUND = {"subject": 1, "date": 2, "time": 3}

# RP-2 §8.2: a position's ranges, in ten-millionths of a degree.
LAT_MAX = 900_000_000
LON_MAX = 1_800_000_000


def field_map(f: dict[str, Any]) -> dict[int, Any]:
    """A field (or a column) as JSON to its CBOR map (RP-2 §8.1)."""
    m: dict[int, Any] = {1: f["n"], 2: f["label"], 3: TYPES[f["type"]]}
    if f.get("required"):
        m[4] = True
    if "choices" in f:
        m[5] = list(f["choices"])
    if "bound" in f:
        m[6] = BOUND[f["bound"]]
    if "max" in f:
        m[7] = f["max"]
    if "scale" in f:
        m[8] = f["scale"]
    if "unit" in f:
        m[9] = f["unit"]
    if "columns" in f:
        m[10] = [field_map(c) for c in f["columns"]]
    return m


def template_map(t: dict[str, Any]) -> dict[int, Any]:
    """A template as JSON (forms/*.json) to the CBOR map RP-2 §8.1 defines."""
    out: dict[int, Any] = {1: t["name"], 2: [field_map(f) for f in t["fields"]]}
    if "note" in t:
        out[3] = t["note"]
    return out


def fingerprint(t: dict[str, Any]) -> bytes:
    """RP-2 §8.1: the first 8 bytes of SHA-256 of the template's deterministic encoding."""
    return hashlib.sha256(cbor.encode(template_map(t))).digest()[:8]


def template_problem(t: dict[str, Any]) -> str | None:
    """RP-2 §8.1: which rule a template breaks, or None when it keeps them all."""
    return _fields_problem(t["fields"], column=False)


def _fields_problem(fields: list[dict[str, Any]], column: bool) -> str | None:
    seen: set[int] = set()
    for f in fields:
        if f["n"] in seen:
            return "a number used twice"
        seen.add(f["n"])
        kind = f["type"]
        if column and kind in {"text", "rows", "heading"}:
            return f"a column of type {kind}"
        if column and "bound" in f:
            return "a bound column"
        if kind == "choice" and not f.get("choices"):
            return "a choice field without choices"
        if kind == "multi" and not 1 <= len(f.get("choices", [])) <= 64:
            return "a multi field without 1 to 64 choices"
        if ("scale" in f or "unit" in f) and kind != "number":
            return "a scale or a unit on a field that is not a number"
        if "scale" in f and not 1 <= f["scale"] <= 9:
            return "a scale that is not 1 to 9"
        if "columns" in f and kind != "rows":
            return "columns on a field that is not rows"
        if kind == "rows":
            if not f.get("columns"):
                return "a rows field without columns"
            why = _fields_problem(f["columns"], column=True)
            if why:
                return why
        if kind == "heading" and (
            f.get("required") or any(k in f for k in ("choices", "bound", "max"))
        ):
            return "a heading with required, choices, bound or max"
    return None


def form_map(template: bytes, values: dict[int, Any]) -> dict[int, Any]:
    """RP-2 §8.2: key 13's value."""
    return {1: template, 2: dict(values)}


def _is_int(v: Any) -> bool:
    return isinstance(v, int) and not isinstance(v, bool)


def _value_problem(f: dict[str, Any], v: Any) -> str | None:
    """RP-2 §8.2: why ``v`` is not a value of field (or column) ``f``, or None."""
    wrong = "a value of the wrong type"
    kind = f["type"]
    if kind in {"line", "text"}:
        if not isinstance(v, str):
            return wrong
        if kind == "line" and "\n" in v:
            return "a line value with a line break"
        if "max" in f and len(v) > f["max"]:
            return "a value longer than the field's max"
        if not v:
            return "an empty value: an empty field is left out"
        if kind == "text" and v.endswith("\n"):
            return "a text value ending with a line break"
    elif kind in {"date", "time", "choice", "datetime", "multi"}:
        if not _is_int(v) or v < 0:
            return wrong
        if kind == "time" and v > 1439:
            return "a time past 23:59"
        if kind == "choice" and v >= len(f["choices"]):
            return "a choice the field does not have"
        if kind == "multi":
            if v == 0:
                return "an empty value: an empty field is left out"
            if v >> len(f["choices"]):
                return "a choice the field does not have"
    elif kind == "number":
        if not _is_int(v):
            return wrong
    elif kind == "check":
        if not isinstance(v, bool):
            return wrong
    elif kind == "position":
        if not isinstance(v, list) or len(v) not in {2, 3} or not all(_is_int(x) for x in v):
            return wrong
        if abs(v[0]) > LAT_MAX or abs(v[1]) > LON_MAX or (len(v) == 3 and v[2] < 1):
            return "a position out of range"
    elif kind == "rows":
        if not isinstance(v, list):
            return wrong
        if not v:
            return "an empty value: an empty field is left out"
        if "max" in f and len(v) > f["max"]:
            return "more rows than the field's max"
        columns = {c["n"]: c for c in f["columns"]}
        for row in v:
            if not isinstance(row, dict):
                return wrong
            if not row:
                return "an empty row"
            for n, cell in row.items():
                if not _is_int(n) or n not in columns:
                    return "a column the field does not have"
                why = _value_problem(columns[n], cell)
                if why:
                    return why
            for c in f["columns"]:
                if c.get("required") and c["n"] not in row:
                    return "a required column missing"
    return None


def problem(t: dict[str, Any], values: dict[int, Any]) -> str | None:
    """RP-2 §8.2: why these values do not fill this template, or None when they do."""
    fields = {f["n"]: f for f in t["fields"]}
    for n, v in values.items():
        f = fields.get(n)
        if f is None:
            return "a field the template does not have"
        if "bound" in f:
            return "a bound field among the values"
        if f["type"] == "heading":
            return "a heading among the values"
        why = _value_problem(f, v)
        if why:
            return why
    for f in t["fields"]:
        if f.get("required") and "bound" not in f and f["n"] not in values:
            return "a required field missing"
    return None


def _date(days: int) -> str:
    return datetime.fromtimestamp(days * 86400, tz=UTC).strftime("%Y-%m-%d")


def _time(minutes: int) -> str:
    return f"{minutes // 60:02d}:{minutes % 60:02d} UTC"


def _datetime(minutes: int) -> str:
    return datetime.fromtimestamp(minutes * 60, tz=UTC).strftime("%Y-%m-%d %H:%M UTC")


def _decimal(value: int, places: int) -> str:
    """An integer read as having ``places`` decimal places, written with exactly that many."""
    sign = "-" if value < 0 else ""
    digits = str(abs(value)).rjust(places + 1, "0")
    return f"{sign}{digits[:-places]}.{digits[-places:]}"


def degrees(e7: int) -> str:
    """RP-2 §8.4: a coordinate in decimal degrees: seven places, less trailing zeros but one."""
    whole = _decimal(e7, 7).rstrip("0")
    return whole + "0" if whole.endswith(".") else whole


def locator(lat: int, lon: int) -> str:
    """RP-2 §8.4: a position's 6-character Maidenhead locator, from its integers."""
    x = min(lon + LON_MAX, 3_599_999_999)
    y = min(lat + LAT_MAX, 1_799_999_999)
    upper = "ABCDEFGHIJKLMNOPQR"
    lower = "abcdefghijklmnopqrstuvwx"
    return (
        upper[x // 200_000_000]
        + upper[y // 100_000_000]
        + str(x % 200_000_000 // 20_000_000)
        + str(y % 100_000_000 // 10_000_000)
        + lower[x % 20_000_000 * 24 // 20_000_000]
        + lower[y % 10_000_000 * 24 // 10_000_000]
    )


def distance(metres: int) -> str:
    """RP-2 §8.4: how far a position may be off: metres under 1000, else kilometres to 0.1."""
    if metres < 1000:
        return f"±{metres} m"
    tenths = (metres + 50) // 100
    return f"±{tenths // 10} km" if tenths % 10 == 0 else f"±{tenths // 10}.{tenths % 10} km"


def position(v: list[int]) -> str:
    """RP-2 §8.4: a position as text."""
    out = f"{degrees(v[0])}, {degrees(v[1])} ({locator(v[0], v[1])})"
    return out + f" {distance(v[2])}" if len(v) == 3 else out


def shown_value(f: dict[str, Any], v: Any) -> str:
    """RP-2 §8.4: a value of field (or column) ``f`` as text; a `rows` value's lines joined."""
    kind = f["type"]
    if kind == "date":
        return _date(v)
    if kind == "time":
        return _time(v)
    if kind == "datetime":
        return _datetime(v)
    if kind == "choice":
        return str(f["choices"][v])
    if kind == "multi":
        return ", ".join(c for i, c in enumerate(f["choices"]) if v >> i & 1)
    if kind == "check":
        return "Yes" if v else "No"
    if kind == "number":
        out = _decimal(v, f["scale"]) if "scale" in f else str(v)
        return f"{out} {f['unit']}" if "unit" in f else out
    if kind == "position":
        return position(v)
    if kind == "rows":
        lines = [" | ".join(c["label"] for c in f["columns"])]
        for row in v:
            cells = [shown_value(c, row[c["n"]]) if c["n"] in row else "" for c in f["columns"]]
            lines.append(" | ".join(cells).rstrip(" "))
        return "\n".join(lines)
    return str(v)


def text(t: dict[str, Any], message: dict[str, Any]) -> str:
    """RP-2 §8.4: the text a form message shows as, with its template."""
    values = message["form"]["values"]
    lines = [t["name"], ""]
    stamp = datetime.fromtimestamp(message["date"], tz=UTC)
    heading: str | None = None
    for f in t["fields"]:
        if f["type"] == "heading":
            heading = f["label"]
            continue
        bound = f.get("bound")
        if bound == "subject":
            v: Any = message["subject"]
            shown = v if v else None
        elif bound == "date":
            shown = stamp.strftime("%Y-%m-%d")
        elif bound == "time":
            shown = stamp.strftime("%H:%M UTC")
        elif f["n"] in values:
            shown = shown_value(f, values[f["n"]])
        else:
            shown = None
        if shown is None:
            continue
        if heading is not None:
            if len(lines) > 2:
                lines.append("")
            lines.append(heading)
            heading = None
        if f["type"] in {"text", "rows"}:
            lines.append(f"{f['label']}:")
            lines.extend(shown.split("\n"))
        else:
            lines.append(f"{f['label']}: {shown}")
    out = "\n".join(lines) + "\n"
    if message.get("body"):
        out += "\n" + message["body"]
    return out


def text_without(message: dict[str, Any]) -> str:
    """RP-2 §8.4: the text a form message shows as, without its template."""
    form = message["form"]
    lines = [f"Form {form['template'].hex()}", ""]
    for n in sorted(form["values"]):
        v = form["values"][n]
        if isinstance(v, bool):
            shown = "Yes" if v else "No"
        elif isinstance(v, str | int):
            shown = str(v)
        else:
            shown = cbor.diagnostic(v)
        if "\n" in shown:
            lines.append(f"Field {n}:")
            lines.extend(shown.split("\n"))
        else:
            lines.append(f"Field {n}: {shown}")
    out = "\n".join(lines) + "\n"
    if message.get("body"):
        out += "\n" + message["body"]
    return out
