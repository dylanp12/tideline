# tideline-sdk

Official Rust client for [TLR/1](https://github.com/dylanp12/tideline/blob/main/spec/tlr-1.md)
— record agent runs, gate high-risk actions on human approval, and verify the
result offline.

```rust
use std::time::Duration;
use tideline_sdk::{Agent, NewEvent, Tideline};

let tl  = Tideline::new("http://localhost:8080", None);
let run = tl.start_run("loan-4821", Agent::new("underwriter", "2.1.0"))
    .subject_ref("applicant-4821")
    .send().await?;

run.record(NewEvent::tool_call("pull_credit_report").content("score=690")).await?;

// Blocks until a person decides. The request and the decision are both in the
// chain by the time this returns.
let decision = run.gate("Approve EUR40,000 loan", Duration::from_secs(7200)).await?;
if decision.is_approved() {
    run.record(NewEvent::decision("loan_approved").content("EUR40,000")).await?;
}
run.complete().await?;

let events = run.verified_events().await?;   // fetch and verify in one step
```

To verify a record without talking to a server — an auditor checking an export —
depend on [`tideline-proto`](https://crates.io/crates/tideline-proto) instead. It
performs no I/O and cannot open a socket.

Licensed under MIT OR Apache-2.0.
