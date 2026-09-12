# tideline-server

The reference TLR/1 implementation: tamper-evident agent run records,
human-approval gates, and resumable output streaming, in one binary.

```bash
cargo install tideline-server
TIDELINE_RECORD_DB=./tideline.db tideline-server    # listens on :8080
```

The server will not start without `TIDELINE_RECORD_DB`, because without it
records live in memory and are lost on restart — and for an audit record that
means the evidence is gone. Set `TIDELINE_EPHEMERAL_RECORDS=1` if you genuinely
want that, as the test suites do.

Streams are held in memory by default, or in Redis for horizontal scale
(`TIDELINE_REDIS_URL`). Note that Redis shares *streams*, not *records*: the
server refuses to start with both unless you confirm it is the only writer, since
a load balancer would otherwise split every run across instances. The protocol it speaks is specified in
[`spec/tlr-1.md`](https://github.com/dylanp12/tideline/blob/main/spec/tlr-1.md);
[`tideline-proto`](https://crates.io/crates/tideline-proto) is the verifier, with
no I/O and no server dependency.

Licensed under MIT OR Apache-2.0.
