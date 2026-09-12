# Contributing

## Running everything

```bash
cargo test --workspace --all-targets     # the engine, the protocol, the CLI
./conformance/run-all.sh                 # every SDK against the same bytes
```

`run-all.sh` skips a surface whose toolchain is missing rather than failing, so
you can work on one language without installing four.

Per surface:

| Surface | |
| --- | --- |
| Rust | `cargo test --workspace --all-targets` |
| TypeScript | `cd sdk/ts && pnpm install && pnpm test` |
| Python | `PYTHONPATH=sdk/python/src pytest sdk/python/tests` |
| Go | `cd sdk/go && go test ./...` |

The SDK suites drive a real `tideline-server`, so run `cargo build --bin
tideline-server` first. They test against the server rather than a mock, because
a mock would agree with whatever the SDK believes — which is the belief under
test.

## The rule that matters

**A change to `conformance/corpus/` is a protocol change.**

Those files pin the bytes every record ever written was hashed against. Editing a
vector in place does not fix an implementation; it silently invalidates every
record in existence. If the canonical form has to change, it takes a new protocol
version, and the old vectors stay.

Regenerate with `cargo run -p tideline-proto --example gen_corpus`, and expect a
diff there to be questioned in review.

## Things worth knowing before you change them

**Never re-serialise a record.** Canon commits to the exact bytes of each event's
`metadata`. Parsing into a generic JSON value and converting back reorders keys
and fails records that are perfectly sound. Rust and Go have raw-preserving types
(`RawValue`, `json.RawMessage`); TypeScript and Python use the byte-span readers
in `parse.ts` and `parse.py`. This is the single most common way to get TLR/1
wrong.

**`conformance/independent_check.py` duplicates the canonical form on purpose.**
Importing the Python SDK there would be tidier and would defeat it: a canon bug
would then live in both the thing under test and the thing testing it.

**Say what you do not know.** If a check cannot prove something — an unsealed
record, an unauthenticated reviewer — the code and the output should say so.
Overclaiming is the failure mode this project exists to avoid.

## Style

- `cargo fmt`, `gofmt`, and Prettier defaults. CI enforces all three.
- Comments explain why, not what. If a line needs a comment to say what it does,
  rewrite the line.
- Tests are named after the behaviour they pin, not the function they call.

## Licence

Contributions are dual-licensed MIT or Apache-2.0, matching the project.
