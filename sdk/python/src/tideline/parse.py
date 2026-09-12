"""A JSON reader that keeps the bytes.

Canon commits to the exact UTF-8 of an event's ``metadata`` (spec/tlr-1.md 4.2).
``json.loads`` throws that away: what comes back is a value, and re-serialising
it reorders keys and renormalises numbers, which changes the digest and fails a
record that is perfectly sound.

So a record is read twice — once by this scanner, to capture each event's
``metadata`` span verbatim, and once by ``json.loads``, for everything else.

The scanner walks members at the top level of each object rather than searching
for ``"metadata":``. A search would find a nested ``metadata`` key first when one
precedes the real member, and hash the wrong bytes.
"""

from __future__ import annotations

from typing import List, Optional

_WS = " \t\n\r"


class ParseError(ValueError):
    def __init__(self, message: str, offset: int) -> None:
        super().__init__(f"{message} (at offset {offset})")
        self.offset = offset


def _skip_ws(s: str, i: int) -> int:
    while i < len(s) and s[i] in _WS:
        i += 1
    return i


def _scan_string(s: str, i: int) -> int:
    """Index just past the closing quote of the string starting at ``i``."""
    if i >= len(s) or s[i] != '"':
        raise ParseError("expected a string", i)
    i += 1
    while i < len(s):
        c = s[i]
        if c == "\\":
            i += 2
            continue
        if c == '"':
            return i + 1
        i += 1
    raise ParseError("unterminated string", i)


def _scan_value(s: str, i: int) -> int:
    """Index just past the value starting at ``i``."""
    if i >= len(s):
        raise ParseError("expected a value", i)
    c = s[i]
    if c == '"':
        return _scan_string(s, i)
    if c in "{[":
        close = "}" if c == "{" else "]"
        depth = 0
        while i < len(s):
            ch = s[i]
            if ch == '"':
                i = _scan_string(s, i)
                continue
            if ch in "{[":
                depth += 1
            elif ch in "}]":
                depth -= 1
                if depth == 0:
                    if ch != close:
                        raise ParseError("mismatched bracket", i)
                    return i + 1
            i += 1
        raise ParseError("unterminated object or array", i)

    start = i
    while i < len(s) and s[i] not in _WS and s[i] not in ",}]":
        i += 1
    if i == start:
        raise ParseError("expected a value", i)
    return i


def raw_member(object_text: str, key: str) -> Optional[str]:
    """The raw text of a top-level member's value, or ``None`` when absent.

    Exported because an auditor verifying an export by hand needs exactly this.
    """
    import json

    i = _skip_ws(object_text, 0)
    if i >= len(object_text) or object_text[i] != "{":
        raise ParseError("expected an object", i)
    i = _skip_ws(object_text, i + 1)
    if i < len(object_text) and object_text[i] == "}":
        return None

    while i < len(object_text):
        key_start = i
        key_end = _scan_string(object_text, i)
        found = json.loads(object_text[key_start:key_end])

        i = _skip_ws(object_text, key_end)
        if i >= len(object_text) or object_text[i] != ":":
            raise ParseError("expected ':'", i)
        i = _skip_ws(object_text, i + 1)

        value_start = i
        value_end = _scan_value(object_text, i)
        if found == key:
            return object_text[value_start:value_end]

        i = _skip_ws(object_text, value_end)
        if i < len(object_text) and object_text[i] == ",":
            i = _skip_ws(object_text, i + 1)
            continue
        if i < len(object_text) and object_text[i] == "}":
            return None
        raise ParseError("expected ',' or '}'", i)
    raise ParseError("unterminated object", i)


def split_array(array_text: str) -> List[str]:
    """The raw text of each element of a top-level JSON array."""
    i = _skip_ws(array_text, 0)
    if i >= len(array_text) or array_text[i] != "[":
        raise ParseError("expected an array", i)
    i = _skip_ws(array_text, i + 1)
    out: List[str] = []
    if i < len(array_text) and array_text[i] == "]":
        return out

    while i < len(array_text):
        start = i
        end = _scan_value(array_text, i)
        out.append(array_text[start:end])
        i = _skip_ws(array_text, end)
        if i < len(array_text) and array_text[i] == ",":
            i = _skip_ws(array_text, i + 1)
            continue
        if i < len(array_text) and array_text[i] == "]":
            return out
        raise ParseError("expected ',' or ']'", i)
    raise ParseError("unterminated array", i)
