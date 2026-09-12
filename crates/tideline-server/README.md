# tideline-server

The reference TLR/1 implementation: tamper-evident agent run records,
human-approval gates, and resumable output streaming, in one binary.

```bash
cargo install tideline-server
tideline-server            # listens on :8080
```

Runs in memory by default, or Redis-backed for horizontal scale
(`TIDELINE_REDIS_URL`). The protocol it speaks is specified in
[`spec/tlr-1.md`](https://github.com/dylanp12/tideline/blob/main/spec/tlr-1.md);
[`tideline-proto`](https://crates.io/crates/tideline-proto) is the verifier, with
no I/O and no server dependency.

Licensed under MIT OR Apache-2.0.
