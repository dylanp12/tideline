# Changelog

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this project uses [semantic versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] — unreleased

The project becomes an open protocol with a reference implementation. TLR/1 —
the Tideline Record Protocol — is specified in `spec/tlr-1.md`, pinned by a
machine-checkable conformance suite, and implemented by four official SDKs.

### Added

- **TLR/1**, specified in `spec/tlr-1.md`: runs and envelopes, a 181-byte
  canonical form, a hash chain, chain-preserving redaction, approvals as chain
  events, and Ed25519 signed checkpoints.
- **`/v1` HTTP binding**: run creation and listing, paginated event reads, live
  tail over SSE, approval gates with expiry, redaction, checkpoints, and
  capability discovery at `/v1/.well-known/tideline`. Writes honour
  `Idempotency-Key`.
- **Official SDKs** — `@tideline/sdk` (TypeScript, with React hooks), `tideline`
  (Python), `tideline-sdk` (Rust), and `github.com/dylanp12/tideline/sdk/go`.
- **`tideline` CLI** — `verify`, `export`, `tail`. `verify` works offline;
  `--no-default-features` builds a binary with no network dependency at all.
- **`tideline-proto`** — the wire types, canonical form, chain verification, and
  checkpoint signatures, with no I/O.
- **Conformance suite** — 8 event vectors, 5 chain cases, a 17-step HTTP
  transcript, an independent implementation of the canonical form, and
  `conformance/run-all.sh` to run every surface at once.
- **Operations** — structured logging via `tracing`, graceful shutdown, a panic
  backstop, record-layer Prometheus metrics, and `TIDELINE_PORT`.

### Changed

- **Breaking: the legacy `/runs` and `/runs/:id/approvals` routes are removed**,
  replaced by `/v1`. They kept approvals in a table that could drift from the
  record it described, and opened a second SQLite connection to the same file
  with no WAL and `.expect` on every statement.
- The record store runs every write in one `BEGIN IMMEDIATE` transaction, with
  WAL and a busy timeout, and returns `Result` rather than panicking.
- Licensed MIT **or** Apache-2.0, adding the patent grant enterprise legal review
  expects. Previously MIT only.
- The repository is a Cargo workspace; the server binary is `tideline-server`,
  freeing `tideline` for the CLI.
- The README leads with the record and the gate. Streaming documentation moved to
  `docs/streaming.md`.
- Removed `cloud/`, a Next.js app for the streaming product, superseded.

### Fixed

- **Record reads were unauthenticated** on the documented Cloud configuration
  (`TIDELINE_AUTH_URL` set, no subscribe token), and the tenant came from a
  caller-supplied `?ns=`. Reads now fall open only when nothing at all is
  configured, and a namespace comes from the credential. `?ns=` is gone from the
  record routes.
- **Records were invisible to the scale-out path.** With `TIDELINE_REDIS_URL`
  set, streams spanned instances but records stayed in process-local SQLite, so a
  load-balanced deployment silently fragmented every run. The server now refuses
  to start in that configuration unless told it is the only writer.
- A `SQLITE_BUSY` panicked the task rather than failing the request.
- Approval gates never expired, so an agent could block forever and leave a
  record that stopped without saying why.
- `cargo publish` failed: the manifest declared no description, licence,
  repository, or MSRV.
