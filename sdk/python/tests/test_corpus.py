"""The Python SDK against the shared corpus."""

import hashlib
import json
import pathlib

import pytest

from tideline import ChainError, parse_event, parse_record, verify_chain

ROOT = pathlib.Path(__file__).resolve().parents[3]
CORPUS = ROOT / "conformance" / "corpus"

EVENTS = json.loads((CORPUS / "events.json").read_text(encoding="utf-8"))
CHAINS = json.loads((CORPUS / "chains.json").read_text(encoding="utf-8"))


def test_corpus_has_not_shrunk():
    assert len(EVENTS["vectors"]) >= 8


@pytest.mark.parametrize("vector", EVENTS["vectors"], ids=lambda v: v["name"])
def test_event_vector_reproduces(vector):
    # parse_event, not json.loads: the metadata bytes must survive.
    event = parse_event(vector["event_json"])
    assert hashlib.sha256(event.canon()).hexdigest() == vector["canon_sha256"]
    assert event.compute_hash(vector["prev_hash"]) == vector["hash"]


@pytest.mark.parametrize("case", CHAINS["cases"], ids=lambda c: c["name"])
def test_chain_case_reaches_its_verdict(case):
    events = parse_record(case["events_json"])
    if case["expect"] == "ok":
        verify_chain(events)
        return
    with pytest.raises(ChainError) as excinfo:
        verify_chain(events)
    assert excinfo.value.reason == case["expect"]
    if "at_seq" in case:
        assert excinfo.value.seq == case["at_seq"]
