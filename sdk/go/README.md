# tideline (Go)

Official Go client for **[TLR/1](https://github.com/dylanp12/tideline/blob/main/spec/tlr-1.md)** — the Tideline Record Protocol.

Record what an AI agent did, gate its high-risk actions on human approval, and
hand anyone a record they can verify without trusting you. EU AI Act Articles
12 and 14.

```bash
go get github.com/dylanp12/tideline/sdk/go
```

Standard library only.

```go
import tideline "github.com/dylanp12/tideline/sdk/go"

tl := tideline.New("http://localhost:8080", os.Getenv("TIDELINE_KEY"))

run, err := tl.StartRun(ctx, tideline.NewRun{
    RunID:      "loan-4821",
    Agent:      tideline.Agent{Name: "underwriter", Version: "2.1.0"},
    SubjectRef: "applicant-4821",           // opaque; never personal data
    Labels:     map[string]string{"product": "personal-loan"},
})

run.Record(ctx, tideline.NewEvent{
    Kind:     tideline.ToolCall,
    Name:     "pull_credit_report",
    Content:  "score=690",
    Metadata: map[string]any{"bureau": "experian", "ms": 410},
})

// Blocks until a person decides. Both halves are in the chain by then.
decision, err := run.Gate(ctx, "Approve EUR40,000 loan", 2*time.Hour)
if decision.Approved() {
    run.Record(ctx, tideline.NewEvent{Kind: tideline.DecisionEvt, Name: "loan_approved"})
}
run.Complete(ctx)
```

A refusal returns a `Resolution`, not an error — an agent has to branch on one
without treating the normal case as a failure.

## Verify without trusting anyone

```go
var events []tideline.Event
json.Unmarshal(data, &events)          // RawMessage keeps the metadata bytes

chain, err := tideline.VerifyChain(events)
// err is a *tideline.ChainError naming the event at fault
```

No server, no network, no credentials. `VerifyChain` detects edited fields,
inserted events, deleted events, and rewritten links. It cannot detect a
truncated tail — a valid prefix is a valid chain — so check `Sealed`, and check
a signed checkpoint, before calling a record complete.

Go is the easiest of the four SDKs to get right here: `json.RawMessage` hands
back the original bytes, so ordinary `json.Unmarshal` preserves what canon
hashes. TypeScript and Python need a byte-span reader for the same job.

One asymmetry worth knowing: `encoding/json` compacts the output of a
`Marshaler`, so a Go client always *sends* compact metadata even when handed a
`json.RawMessage` containing whitespace. That is harmless — the server hashes
the bytes it receives — but it means a Go-written record reads slightly
differently from one written elsewhere.

## Conformance

This module reproduces every vector in
[`conformance/corpus/`](https://github.com/dylanp12/tideline/tree/main/conformance)
and completes the HTTP transcript against a live server, as do the TypeScript,
Python, and Rust SDKs.

```bash
cargo build --bin tideline-server    # the tests drive a real server
go test ./...
```

Licensed under MIT OR Apache-2.0.
