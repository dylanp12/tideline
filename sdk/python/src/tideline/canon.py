"""The canonical form, and the chain built on it."""

from __future__ import annotations

import hashlib
import json
import struct
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional

from .parse import ParseError, raw_member, split_array

CANON_LEN = 181
_DOMAIN = b"tlr1\n"
ZERO = b"\x00" * 32
ZERO_HEX = "0" * 64

EVENT_KINDS = (
    "run_started",
    "message",
    "model_call",
    "tool_call",
    "decision",
    "redaction",
    "run_finished",
)

#: The fields a redaction may erase. ``seq``, ``ts`` and ``kind`` are structural.
REDACTABLE = ("role", "name", "content", "metadata")


def _sha256(data: bytes) -> bytes:
    return hashlib.sha256(data).digest()


@dataclass
class Event:
    """One event on the record.

    ``metadata`` is the parsed value, for reading. ``metadata_raw`` is the exact
    text the server sent, and is what canon hashes. Never rebuild one from the
    other.
    """

    seq: int
    ts: int
    kind: str
    role: Optional[str] = None
    name: Optional[str] = None
    content: Optional[str] = None
    metadata: Any = None
    metadata_raw: Optional[str] = None
    redacted: Dict[str, str] = field(default_factory=dict)
    prev_hash: str = ZERO_HEX
    hash: str = ZERO_HEX

    def _digest(self, name: str, value: Optional[str]) -> bytes:
        """present -> SHA-256 of the value; erased -> the retained digest;
        never set -> 32 zero bytes.

        A present value always wins, so a forged ``redacted`` map cannot restate
        the hash of an event that still carries its content.
        """
        if value is not None:
            return _sha256(value.encode("utf-8"))
        kept = self.redacted.get(name)
        if kept:
            return bytes.fromhex(kept)
        return ZERO

    def canon(self) -> bytes:
        """The canonical 181-byte encoding of this event."""
        out = (
            _DOMAIN
            + struct.pack(">Q", self.seq)
            + struct.pack(">Q", self.ts)
            + _sha256(self.kind.encode("utf-8"))
            + self._digest("role", self.role)
            + self._digest("name", self.name)
            + self._digest("content", self.content)
            + self._digest("metadata", self.metadata_raw)
        )
        if len(out) != CANON_LEN:
            raise AssertionError(f"canon is {len(out)} bytes, expected {CANON_LEN}")
        return out

    def compute_hash(self, prev_hash_hex: str) -> str:
        """This event's hash, committing to its predecessor."""
        return hashlib.sha256(self.canon() + bytes.fromhex(prev_hash_hex)).hexdigest()


def parse_event(text: str) -> Event:
    """Parse one event from its source text, keeping the metadata bytes."""
    raw = json.loads(text)
    kind = raw.get("kind")
    if kind not in EVENT_KINDS:
        # Defaulting would silently relabel the event.
        raise ParseError(f"unknown event kind {kind!r}", 0)
    metadata_raw = raw_member(text, "metadata")
    return Event(
        seq=raw["seq"],
        ts=raw["ts"],
        kind=kind,
        role=raw.get("role"),
        name=raw.get("name"),
        content=raw.get("content"),
        metadata=raw.get("metadata"),
        # A null metadata member is absent, not an empty claim.
        metadata_raw=None if metadata_raw == "null" else metadata_raw,
        redacted=raw.get("redacted") or {},
        prev_hash=raw.get("prev_hash", ZERO_HEX),
        hash=raw.get("hash", ZERO_HEX),
    )


def parse_record(text: str) -> List[Event]:
    """Parse a whole record from the response text.

    Always use this on a record. ``json.loads`` alone discards the metadata bytes
    and every event will then fail verification.
    """
    return [parse_event(t) for t in split_array(text)]
