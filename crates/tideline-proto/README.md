# tideline-proto

TLR/1 wire types, canonical hashing, chain verification, and signed checkpoints.

This crate performs no I/O. An auditor verifying an exported agent-run record
should compile nothing that can open a socket.

```rust
use tideline_proto::{verify_chain, RunEvent};

let events: Vec<RunEvent> = serde_json::from_str(&std::fs::read_to_string("record.json")?)?;
let ok = verify_chain(&events)?;
println!("{} events verified, head {}", ok.len, ok.head_hash.to_hex());
```

`verify_chain` detects edited fields, inserted events, deleted events, and
rewritten links, and names the event at fault. It cannot detect a truncated
tail — a valid prefix is a valid chain — which is what `Checkpoint` is for.

The protocol is specified in [`spec/tlr-1.md`](https://github.com/dylanp12/tideline/blob/main/spec/tlr-1.md)
and pinned by the vectors in [`conformance/`](https://github.com/dylanp12/tideline/tree/main/conformance).

Licensed under MIT OR Apache-2.0.
