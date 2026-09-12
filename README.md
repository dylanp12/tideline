# Tideline

[![CI](https://github.com/dylanp12/tideline/actions/workflows/ci.yml/badge.svg)](https://github.com/dylanp12/tideline/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/tideline-proto.svg)](https://crates.io/crates/tideline-proto)
[![npm](https://img.shields.io/npm/v/@tideline/sdk.svg)](https://www.npmjs.com/package/@tideline/sdk)
[![PyPI](https://img.shields.io/pypi/v/tideline.svg)](https://pypi.org/project/tideline/)

**A record of what your AI agents did, that anyone can verify without trusting you.**

Tideline is an open protocol and a reference implementation for the durable,
tamper-evident record of an agent run, and for the human-approval gates over it.
Official SDKs in TypeScript, Python, Rust, and Go. Self-hostable, Apache-2.0 and
MIT, written in Rust.

```console
$ tideline verify evidence.json
8 events verified
head      1711f9a6737029f6…f96f53e885952 (seq 7)
sealed    yes
```

No server. No credentials. No network. That is the point.

## Why

Enterprises are putting autonomous agents into production faster than they can
govern them, and in regulated industries that collides with law. Under the EU AI
Act, Annex III high-risk obligations — **Article 12** (record-keeping) and
**Article 14** (human oversight) — apply from **2 December 2027**, postponed from
August 2026 by the Digital Omnibus adopted in June 2026. Breaches reach **€15M or
3% of worldwide turnover** (Art 99(4)).

The part that sets the timetable is not the deadline. It is that **records cannot
be backfilled**. A decision an agent makes today is provable in 2027 only if it
was recorded today.

What exists does not fit:

- **Observability tools** are post-hoc trace viewers built for developers
  debugging. They are not evidence, and they are not designed to be read by
  someone who does not trust the vendor.
- **GRC platforms** sit at the policy layer — risk registers, model cards — and
  never see the agent's actual inputs, outputs, or decisions.
- **Platform-native tracing** (Vercel, Cloudflare, the model vendors) is
  excellent and single-vendor by design. A team running agents across several
  models and frameworks has no one pane, and the platforms have no reason to
  build one.

Tideline occupies the position none of them do: **vendor-neutral, in the
execution path, and independently verifiable**.

## The idea

A run is an append-only chain. Each event commits to the one before it, so
altering any of them is detectable by anyone holding the record — including
someone who does not trust the server that served it.

```
run_started ─► message ─► tool_call ─► approval_requested ─► approval_resolved ─► decision ─► run_finished
     │            │           │                │                    │                │            │
   hash₀ ────►  hash₁ ────► hash₂ ────────►  hash₃ ──────────────► hash₄ ────────► hash₅ ────► hash₆
```

Three properties follow, and each one is a test in this repository:

**Alteration is detectable.** Change any field of any event and verification
fails, naming the event.

**Oversight lives inside the record.** An approval gate is two events in the
chain, not a table beside it, so the Article 14 trail cannot drift out of step
with the Article 12 record.

**Erasure does not destroy evidence.** Canon commits to each field by digest, so
honouring a GDPR Article 17 request erases the value, keeps the digest, and the
chain still verifies. Retention and erasure stop being mutually exclusive.

A hash chain cannot detect a *truncated* tail — a valid prefix is a valid chain —
so the server signs periodic checkpoints, and the tooling says plainly when a
record is unsealed and uncheckpointed rather than implying more than it knows.

## Quickstart

```bash
cargo install tideline-server

# Records are the product, so the server will not start without somewhere
# durable to keep them. Point it at a file, or say you want them ephemeral.
TIDELINE_RECORD_DB=./tideline.db tideline-server        # listens on :8080
```

```ts
import { Tideline } from "@tideline/sdk";

const tl  = new Tideline({ url: "http://localhost:8080" });
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

// Does not return until a person decides. By then the request and the decision
// are both in the chain.
const decision = await run.gate({
  action: "Approve EUR40,000 loan for applicant #4821",
  expiresIn: 7200,
});

if (decision.approved) {
  await run.record({ kind: "decision", name: "loan_approved", content: "EUR40,000" });
}
await run.complete();
```

See the whole arc, including verification, in one command:

```bash
./demo/oversight-demo.sh
```

## SDKs

| Language | Install | Source |
| --- | --- | --- |
| TypeScript | `npm install @tideline/sdk` | [`sdk/ts`](sdk/ts) |
| Python | `pip install tideline` | [`sdk/python`](sdk/python) |
| Rust | `cargo add tideline-sdk` | [`crates/tideline-sdk`](crates/tideline-sdk) |
| Go | `go get github.com/dylanp12/tideline/sdk/go` | [`sdk/go`](sdk/go) |
| CLI | `cargo install tideline-cli` | [`crates/tideline-cli`](crates/tideline-cli) |

All four expose the same shape, so examples translate line for line, and all
four are held to the same bytes by [`conformance/`](conformance).

For verification alone — an auditor checking an export, with no server in the
picture — use [`tideline-proto`](crates/tideline-proto), which performs no I/O,
or `cargo install tideline-cli --no-default-features`, which produces a binary
that cannot open a socket.

## The protocol

**[TLR/1](spec/tlr-1.md)** is specified independently of this implementation, so
a record produced here can be verified anywhere, and someone else can implement
it. Conformance is machine-checkable, not a claim:

```console
$ ./conformance/run-all.sh
surface        result detail
independent    PASS    8 vectors
rust           PASS  161 tests
ts             PASS   45 tests
python         PASS   44 tests
go             PASS   32 tests

Every surface agrees on the same bytes.
```

`independent` is a second implementation of the canonical form that shares no
code with any SDK. Without it, replaying the corpus would only prove the code
agrees with itself.

### API

| Method and path | Purpose |
| --- | --- |
| `POST /v1/runs` · `GET /v1/runs` | open a run · list runs, paginated and filtered |
| `POST /v1/runs/:id/events` | append → `{seq, ts, hash, prev_hash}`, honouring `Idempotency-Key` |
| `GET /v1/runs/:id/events` | read the record, paginated |
| `GET /v1/runs/:id/watch` | follow it live over SSE |
| `POST /v1/runs/:id/approvals` … `/resolve` | open a gate · the reviewer's queue · a person decides |
| `POST /v1/runs/:id/complete` | seal the run and checkpoint it |
| `GET /v1/runs/:id/checkpoint` | the latest signed checkpoint |
| `POST /v1/runs/:id/redactions` | erase fields, keep the chain |
| `GET /v1/.well-known/tideline` | capabilities, versions, signing keys |

Full semantics, including the canonical form byte by byte, are in
[`spec/tlr-1.md`](spec/tlr-1.md).

## Streaming

The same engine streams model output to clients, which is what `watch` is built
on. Resumable over SSE and WebSocket, fan-out to many subscribers, replay for
late joiners, and slow-client isolation; in-memory by default, Redis-backed for
horizontal scale.

```bash
curl -XPOST localhost:8080/streams/demo --data 'Hello '
curl -N localhost:8080/streams/demo                     # id: 0 / data: Hello
curl -N -H 'Last-Event-ID: 0' localhost:8080/streams/demo   # resumes exactly
```

See [`docs/streaming.md`](docs/streaming.md).

## Production

- **Authentication** — `TIDELINE_PUBLISH_TOKEN` for writes,
  `TIDELINE_SUBSCRIBE_TOKEN` for reads, or `TIDELINE_AUTH_URL` for per-tenant
  keys verified against a control plane. Record reads fall open only when
  *nothing* is configured; a tenant's namespace comes from its credential and
  never from a request parameter.
- **Storage** — SQLite by default (WAL, single connection). With
  `TIDELINE_REDIS_URL` set, the server refuses to start unless records are
  explicitly single-writer, because a load balancer would otherwise split every
  run across instances.
- **Checkpoints** — set `TIDELINE_CHECKPOINT_KEY` to the base64 Ed25519 seed.
  Without one the server generates an ephemeral key and says so: checkpoints
  signed with it cannot be verified after a restart.
- **Observability** — `GET /metrics` (Prometheus) covers runs, appends, append
  failures, gates by decision, redactions, and checkpoint age. Structured logs
  via `RUST_LOG`, JSON with `TIDELINE_LOG_FORMAT=json`.
- **Deploy** — `docker build -t tideline . && docker run -p 8080:8080 tideline`,
  or `fly deploy` with the included `fly.toml`.

## What this does not claim

The chain proves a record was not altered after it was written. It does not prove
the agent reported its actions honestly in the first place — which is why capture
belongs in the execution path, and why `attested: false` appears on any approval
whose reviewer the server could not authenticate.

A record with no checkpoint can be truncated undetectably. The tooling says so.

## Related

Tideline is the open protocol and the reference implementation.
[Paraph](https://paraphhq.vercel.app) is a commercial service that speaks it,
adding external anchoring of checkpoints to independent timestamp authorities,
evidence packs, retention and legal hold, and a multi-reviewer control room. The
open half stays useful on its own: self-host it and never talk to anyone.

## Licence

[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
