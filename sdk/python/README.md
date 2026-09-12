# tideline

Official Python client for **[TLR/1](https://github.com/dylanp12/tideline/blob/main/spec/tlr-1.md)** — the Tideline Record Protocol.

Record what an AI agent did, gate its high-risk actions on human approval, and
hand anyone a record they can verify without trusting you. EU AI Act Articles
12 and 14.

```bash
pip install tideline
```

No dependencies. A compliance team installing a verifier should not be
installing a dependency tree with it.

## Record a run

```python
from tideline import Tideline

tl  = Tideline("http://localhost:8080", key=os.environ.get("TIDELINE_KEY"))
run = tl.start_run(
    "loan-4821",
    agent_name="underwriter",
    agent_version="2.1.0",
    subject_ref="applicant-4821",          # opaque; never personal data
    labels={"product": "personal-loan", "jurisdiction": "IE"},
)

run.record(
    "tool_call",
    name="pull_credit_report",
    content="score=690 utilisation=42%",
    metadata={"bureau": "experian", "ms": 410},
)
```

## Gate a high-risk action

`gate()` does not return until a person decides. By then, both the request and
the decision are already in the chain.

```python
decision = run.gate("Approve EUR40,000 loan for applicant #4821", expires_in=7200)

if decision.approved:
    run.record("decision", name="loan_approved", content="EUR40,000")
run.complete()
```

A refusal returns rather than raising — an agent has to branch on one without
wrapping the normal case in exception handling. A gate nobody answers resolves
as `expired`, and that is recorded too: a record that simply stops is
indistinguishable from one that was truncated.

## Verify without trusting anyone

```python
from tideline import parse_record, verify_chain

events = parse_record(open("evidence.json", encoding="utf-8").read())
chain  = verify_chain(events)      # raises ChainError, naming the bad event

print(f"{chain.length} events verified, sealed: {chain.sealed}")
```

No server, no network, no credentials. `verify_chain` detects edited fields,
inserted events, deleted events, and rewritten links. It cannot detect a
truncated tail — a valid prefix is a valid chain — so check `sealed`, and check
a signed checkpoint, before calling a record complete.

## One rule that matters

**Read a record with `parse_record`, never `json.loads`.**

Canon hashes the exact bytes of each event's `metadata`. `json.loads` discards
them, and re-serialising reorders keys — which changes the digest and fails a
record that is perfectly sound. `parse_record` keeps the bytes. The client's own
methods already do the right thing; this only matters when you read a record
from a file or another source yourself.

## Conformance

This package reproduces every vector in
[`conformance/corpus/`](https://github.com/dylanp12/tideline/tree/main/conformance)
and completes the HTTP transcript against a live server, as do the TypeScript,
Rust, and Go SDKs. They are held to the same bytes.

Licensed under MIT OR Apache-2.0.
