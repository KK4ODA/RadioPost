# SPDX-License-Identifier: MIT OR Apache-2.0
"""RP-2 §8: forms. A template, its fingerprint, a form's values checked against it, and its text."""

from __future__ import annotations

import hashlib
from datetime import UTC, datetime
from typing import Any

from . import cbor

# RP-2 §8.1: a field's types, by number.
TYPES = {"line": 0, "text": 1, "date": 2, "time": 3, "number": 4, "choice": 5, "check": 6}
# RP-2 §8.1: what a bound field takes from the message.
BOUND = {"subject": 1, "date": 2, "time": 3}


def template_map(t: dict[str, Any]) -> dict[int, Any]:
    """A template as JSON (forms/*.json) to the CBOR map RP-2 §8.1 defines."""
    fields = []
    for f in t["fields"]:
        m: dict[int, Any] = {1: f["n"], 2: f["label"], 3: TYPES[f["type"]]}
        if f.get("required"):
            m[4] = True
        if "choices" in f:
            m[5] = list(f["choices"])
        if "bound" in f:
            m[6] = BOUND[f["bound"]]
        if "max" in f:
            m[7] = f["max"]
        fields.append(m)
    out: dict[int, Any] = {1: t["name"], 2: fields}
    if "note" in t:
        out[3] = t["note"]
    return out


def fingerprint(t: dict[str, Any]) -> bytes:
    """RP-2 §8.1: the first 8 bytes of SHA-256 of the template's deterministic encoding."""
    return hashlib.sha256(cbor.encode(template_map(t))).digest()[:8]


def form_map(template: bytes, values: dict[int, Any]) -> dict[int, Any]:
    """RP-2 §8.2: key 13's value."""
    return {1: template, 2: dict(values)}


def problem(t: dict[str, Any], values: dict[int, Any]) -> str | None:
    """RP-2 §8.2: why these values do not fill this template, or None when they do."""
    fields = {f["n"]: f for f in t["fields"]}
    for n, v in values.items():
        f = fields.get(n)
        if f is None:
            return "a field the template does not have"
        if "bound" in f:
            return "a bound field among the values"
        kind = f["type"]
        if kind in {"line", "text"}:
            if not isinstance(v, str):
                return "a value of the wrong type"
            if kind == "line" and "\n" in v:
                return "a line value with a line break"
            if "max" in f and len(v) > f["max"]:
                return "a value longer than the field's max"
            if not v:
                return "an empty value: an empty field is left out"
            if kind == "text" and v.endswith("\n"):
                return "a text value ending with a line break"
        elif kind in {"date", "time", "choice"}:
            if isinstance(v, bool) or not isinstance(v, int) or v < 0:
                return "a value of the wrong type"
            if kind == "time" and v > 1439:
                return "a time past 23:59"
            if kind == "choice" and v >= len(f["choices"]):
                return "a choice the field does not have"
        elif kind == "number":
            if isinstance(v, bool) or not isinstance(v, int):
                return "a value of the wrong type"
        elif kind == "check":
            if not isinstance(v, bool):
                return "a value of the wrong type"
    for f in t["fields"]:
        if f.get("required") and "bound" not in f and f["n"] not in values:
            return "a required field missing"
    return None


def _date(days: int) -> str:
    return datetime.fromtimestamp(days * 86400, tz=UTC).strftime("%Y-%m-%d")


def _time(minutes: int) -> str:
    return f"{minutes // 60:02d}:{minutes % 60:02d} UTC"


def _shown(f: dict[str, Any], v: Any) -> str:
    kind = f["type"]
    if kind == "date":
        return _date(v)
    if kind == "time":
        return _time(v)
    if kind == "choice":
        return f["choices"][v]
    if kind == "check":
        return "Yes" if v else "No"
    return str(v)


def text(t: dict[str, Any], message: dict[str, Any]) -> str:
    """RP-2 §8.4: the text a form message shows as, with its template."""
    values = message["form"]["values"]
    lines = [t["name"], ""]
    stamp = datetime.fromtimestamp(message["date"], tz=UTC)
    for f in t["fields"]:
        bound = f.get("bound")
        if bound == "subject":
            v: Any = message["subject"]
            shown = v if v else None
        elif bound == "date":
            shown = stamp.strftime("%Y-%m-%d")
        elif bound == "time":
            shown = stamp.strftime("%H:%M UTC")
        elif f["n"] in values:
            shown = _shown(f, values[f["n"]])
        else:
            shown = None
        if shown is None:
            continue
        if f["type"] == "text":
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
        shown = ("Yes" if v else "No") if isinstance(v, bool) else str(v)
        if isinstance(v, str) and "\n" in v:
            lines.append(f"Field {n}:")
            lines.extend(v.split("\n"))
        else:
            lines.append(f"Field {n}: {shown}")
    out = "\n".join(lines) + "\n"
    if message.get("body"):
        out += "\n" + message["body"]
    return out
