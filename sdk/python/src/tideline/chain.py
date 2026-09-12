"""Chain verification."""

from __future__ import annotations

from dataclasses import dataclass
from typing import List, Optional, Sequence

from .canon import ZERO_HEX, Event


class ChainError(Exception):
    """A record that does not verify, and where."""

    def __init__(self, reason: str, seq: Optional[int], message: str) -> None:
        super().__init__(message)
        self.reason = reason
        self.seq = seq


@dataclass
class ChainOk:
    length: int
    head_seq: int
    head_hash: str
    #: True when the record ends in ``run_finished``.
    sealed: bool


def verify_chain(events: Sequence[Event]) -> ChainOk:
    """Verify an ordered run record end to end.

    Detects edited fields, inserted events, deleted events, and rewritten links,
    and names the event at fault. It cannot detect truncation of the tail — a
    valid prefix is a valid chain — which is what checkpoints are for. Check
    ``sealed``, and a checkpoint, before calling a record complete.
    """
    if not events:
        raise ChainError("empty", None, "the record is empty")
    if events[0].kind != "run_started":
        raise ChainError(
            "not_run_started",
            0,
            f"a record must open with run_started, got {events[0].kind}",
        )

    prev = ZERO_HEX
    for i, e in enumerate(events):
        if e.seq != i:
            raise ChainError("seq_gap", i, f"expected seq {i}, found {e.seq}")
        if e.prev_hash != prev:
            raise ChainError(
                "prev_mismatch",
                e.seq,
                f"event {e.seq} points at {e.prev_hash[:16]}…, expected {prev[:16]}…",
            )
        computed = e.compute_hash(prev)
        if computed != e.hash:
            raise ChainError(
                "hash_mismatch",
                e.seq,
                f"event {e.seq} was altered: it hashes to {computed[:16]}…, "
                f"but claims {e.hash[:16]}…",
            )
        prev = computed

    last: Event = events[-1]
    return ChainOk(
        length=len(events),
        head_seq=last.seq,
        head_hash=last.hash,
        sealed=last.kind == "run_finished",
    )


__all__: List[str] = ["ChainError", "ChainOk", "verify_chain"]
