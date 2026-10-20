# @tideline/sdk

Official TypeScript client for **[TLR/1](https://github.com/dylanp12/tideline/blob/main/spec/tlr-1.md)** — the Tideline Record Protocol.

Record what an AI agent did, gate its high-risk actions on human approval, and
hand anyone a record they can verify without trusting you. EU AI Act Articles
12 and 14.

```bash
npm install @tideline/sdk
```

## Record a run

```ts
import { Tideline } from "@tideline/sdk";

const tl  = new Tideline({ url: "http://localhost:8080", key: process.env.TIDELINE_KEY });
const run = await tl.startRun({
  runId: "loan-4821",
  agent: { name: "underwriter", version: "2.1.0" },
  subjectRef: "applicant-4821",          // opaque; never personal data
  labels: { product: "personal-loan", jurisdiction: "IE" },
});

await run.record({
  kind: "tool_call",
  name: "pull_credit_report",
  content: "score=690 utilisation=42%",
  metadata: { bureau: "experian", ms: 410 },
});
```

## Gate a high-risk action

`gate()` does not return until a person decides. By the time it does, both the
request and the decision are already in the chain.

```ts
const decision = await run.gate({
  action: "Approve EUR40,000 loan for applicant #4821",
  expiresIn: 7200,
});

if (decision.approved) {
  await run.record({ kind: "decision", name: "loan_approved", content: "EUR40,000" });
}
await run.complete();
```

A refusal resolves rather than throwing — an agent has to be able to branch on
one without wrapping the normal case in error handling. A gate nobody answers
resolves as `expired`, and that is recorded too: a record that simply stops is
indistinguishable from one that was truncated.

## Verify without trusting anyone

```ts
import { parseRecord, verifyChain } from "@tideline/sdk";

const events = parseRecord(await fs.readFile("evidence.json", "utf8"));
const chain  = verifyChain(events);   // throws ChainError, naming the bad event

console.log(`${chain.len} events verified, sealed: ${chain.sealed}`);
```

No server, no network, no credentials. `verifyChain` detects edited fields,
inserted events, deleted events, and rewritten links. It cannot detect a
truncated tail — a valid prefix is a valid chain — so check `sealed`, and check
a signed checkpoint, before calling a record complete.

## One rule that matters

**Read a record with `parseRecord`, never `JSON.parse`.**

Canon hashes the exact bytes of each event's `metadata`. `JSON.parse` discards
them, and re-serialising reorders keys — which changes the digest and fails a
record that is perfectly sound. `parseRecord` keeps the bytes. The client's own
methods already do the right thing; this only matters when you read a record
from a file or another source yourself.

## React

```tsx
import { useRun, useApprovalQueue } from "@tideline/sdk/react";

function Oversight({ client, runId }) {
  const { events, verified, verify } = useRun(client, runId);
  const { gates, resolve } = useApprovalQueue(client, runId);

  return (
    <>
      {gates.map((g) => (
        <button key={g.seq} onClick={() => resolve(g.seq, "approved", "Jane Okafor")}>
          Approve: {g.action}
        </button>
      ))}
      <ol>{events.map((e) => <li key={e.seq}>{e.kind}: {e.content}</li>)}</ol>
      <button onClick={verify}>Verify ({String(verified)})</button>
    </>
  );
}
```

`useStream` is also here, for following transient token output with automatic
resume.

## Conformance

This package reproduces every vector in
[`conformance/corpus/`](https://github.com/dylanp12/tideline/tree/main/conformance)
and completes the HTTP transcript against a live server, as do the Rust, Python,
and Go SDKs. They are held to the same bytes.

Licensed under MIT OR Apache-2.0.
