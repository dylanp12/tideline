"""The Python client against a live reference server."""

import os
import threading
import time

import pytest

from tideline import ChainError, Tideline, TidelineError, verify_chain

COUNTER = iter(range(10_000))


@pytest.fixture
def tl(server):
    return Tideline(server)


def new_run(tl):
    return tl.start_run(
        f"py-{os.getpid()}-{next(COUNTER)}",
        "underwriter",
        "2.1.0",
        subject_ref="applicant-4821",
        labels={"product": "personal-loan"},
    )


def test_records_a_run_and_verifies_it(tl):
    run = new_run(tl)
    run.record(
        "tool_call",
        role="tool",
        name="pull_credit_report",
        content="score=690",
        metadata={"bureau": "experian", "ms": 410},
    )
    run.complete()

    events, chain = run.verified_events()
    assert chain.sealed
    assert chain.length == 3
    assert events[1].kind == "tool_call"
    # The metadata survived byte for byte, which is the only reason it verifies.
    assert events[1].metadata_raw == '{"bureau":"experian","ms":410}'


def test_a_record_verifies_whatever_json_formatting_the_client_used(tl):
    """The protocol commits to bytes, not to a normalisation of them.

    Two clients may send equivalent JSON with different spacing and key order.
    Each record verifies against the bytes that client actually sent, which is
    why no canonicalisation scheme is needed — and why a client must never
    re-serialise a record it reads back.
    """
    import json
    import urllib.request

    run = new_run(tl)
    for spacing in ('{"z":1,"a":2}', '{ "z" : 1 , "a" : 2 }'):
        body = f'{{"kind":"model_call","metadata":{spacing}}}'.encode()
        req = urllib.request.Request(
            f"{tl.base}/v1/runs/{run.id}/events",
            data=body,
            headers={"content-type": "application/json"},
            method="POST",
        )
        with urllib.request.urlopen(req) as res:
            assert res.status == 200

    events, chain = run.verified_events()
    assert chain.length == 3
    # Each event kept the exact bytes it arrived with.
    assert events[1].metadata_raw == '{"z":1,"a":2}'
    assert events[2].metadata_raw == '{ "z" : 1 , "a" : 2 }'
    assert json.loads(events[1].metadata_raw) == json.loads(events[2].metadata_raw)


def test_envelope_reports_the_head(tl):
    run = new_run(tl)
    run.record("message", content="x")
    env = run.envelope()
    assert env["head_seq"] == 1
    assert env["agent"]["name"] == "underwriter"


def test_repeated_idempotency_key_appends_once(tl):
    run = new_run(tl)
    a = run.record("message", content="once", idempotency_key="k-1")
    b = run.record("message", content="once", idempotency_key="k-1")
    assert a.seq == b.seq and a.hash == b.hash
    assert len(run.events()) == 2


def test_gate_blocks_until_a_reviewer_decides(tl):
    run = new_run(tl)
    reviewer = tl.run(run.id)

    def review():
        for _ in range(100):
            pending = reviewer.approvals()
            if pending:
                reviewer.resolve(pending[0].seq, "approved", "Jane Okafor", "within policy")
                return
            time.sleep(0.02)

    threading.Thread(target=review, daemon=True).start()

    decision = run.gate("Approve EUR40,000 loan", expires_in=60)
    assert decision.approved
    assert decision.reviewer == "Jane Okafor"
    # No control plane configured, so the server cannot vouch for who that was.
    assert decision.attested is False
    verify_chain(run.events())


def test_a_rejection_returns_rather_than_raising(tl):
    # An agent must be able to branch on a refusal.
    run = new_run(tl)
    opened = run.open_gate("Approve EUR900,000", expires_in=60)
    run.resolve(opened["seq"], "rejected", "Risk", "exceeds mandate")

    decision = run.await_gate(opened["seq"])
    assert decision.decision == "rejected"
    assert not decision.approved


def test_second_resolution_is_a_conflict(tl):
    run = new_run(tl)
    opened = run.open_gate("act", expires_in=60)
    run.resolve(opened["seq"], "approved")
    with pytest.raises(TidelineError) as e:
        run.resolve(opened["seq"], "rejected")
    assert e.value.is_conflict


def test_tampering_is_detected_and_located(tl):
    run = new_run(tl)
    for i in range(4):
        run.record("message", content=f"m{i}")

    events = run.events()
    events[3].content = "altered after the fact"

    with pytest.raises(ChainError) as e:
        verify_chain(events)
    assert e.value.reason == "hash_mismatch"
    assert e.value.seq == 3


def test_redaction_keeps_the_record_verifiable(tl):
    run = new_run(tl)
    run.record("tool_call", name="kyc", content="applicant dossier")
    run.redact(1, ["content"], "GDPR Art 17 request #55")

    events, _ = run.verified_events()
    assert events[1].content is None
    assert "content" in events[1].redacted


def test_checkpoint_on_seal_matches_the_published_key(tl):
    run = new_run(tl)
    assert run.checkpoint() is None
    run.complete()

    cp = run.checkpoint()
    assert cp is not None
    assert cp["key_id"] == tl.well_known()["keys"][0]["id"]


def test_watch_streams_the_record(tl):
    run = new_run(tl)
    run.record("message", content="first")

    seen = []
    for event in run.watch():
        seen.append(event)
        if len(seen) == 2:
            break
    assert seen[0].kind == "run_started"
    assert seen[1].content == "first"


def test_unknown_run_is_not_found(tl):
    with pytest.raises(TidelineError) as e:
        tl.run("nope-nope").envelope()
    assert e.value.is_not_found
