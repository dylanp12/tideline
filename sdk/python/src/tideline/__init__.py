"""Official Python client for TLR/1 — the Tideline Record Protocol.

Record what an AI agent did, gate its high-risk actions on human approval, and
hand anyone a record they can verify without trusting you.

    from tideline import Tideline

    tl  = Tideline("http://localhost:8080")
    run = tl.start_run("loan-4821", "underwriter", "2.1.0", subject_ref="applicant-4821")

    run.record("tool_call", name="pull_credit_report", content="score=690")

    # Blocks until a person decides. Both halves are in the chain by then.
    decision = run.gate("Approve EUR40,000 loan", expires_in=7200)
    if decision.approved:
        run.record("decision", name="loan_approved", content="EUR40,000")
    run.complete()

    events, chain = run.verified_events()

``verify_chain`` and ``parse_record`` need no server and no network.
"""

from .canon import (
    CANON_LEN,
    EVENT_KINDS,
    REDACTABLE,
    ZERO_HEX,
    Event,
    parse_event,
    parse_record,
)
from .chain import ChainError, ChainOk, verify_chain
from .client import Appended, Gate, Resolution, Run, Tideline, TidelineError
from .parse import ParseError, raw_member, split_array

__version__ = "0.2.0"

__all__ = [
    "Appended",
    "CANON_LEN",
    "ChainError",
    "ChainOk",
    "EVENT_KINDS",
    "Event",
    "Gate",
    "ParseError",
    "REDACTABLE",
    "Resolution",
    "Run",
    "Tideline",
    "TidelineError",
    "ZERO_HEX",
    "parse_event",
    "parse_record",
    "raw_member",
    "split_array",
    "verify_chain",
]
