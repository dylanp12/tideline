# Tideline Record Protocol, Version 1 (TLR/1)

**Status:** Draft · **Protocol version:** 1 · **Date:** 2026-09-12

TLR/1 defines a tamper-evident, vendor-neutral record of an AI agent run and the
human-approval gates over it. It exists so that a record produced by one vendor
can be verified by anyone, using any implementation, without trusting the vendor
that produced it or having a network connection to them.

The key words MUST, MUST NOT, REQUIRED, SHALL, SHOULD, SHOULD NOT, and MAY are to
be interpreted as described in RFC 2119.

---

## 1. Scope and conformance

TLR/1 covers the record: its structure, its integrity, and the HTTP interface for
writing and reading it. It does not cover storage, retention policy, access
control models, or the anchoring of checkpoints to external authorities. Those
are implementation and product concerns.

A **server** conforms when it satisfies this document and passes the transcript in
`conformance/`. A **client** conforms when it reproduces every vector in
`conformance/corpus/events.json`, reaches the stated verdict for every case in
`conformance/corpus/chains.json`, and completes the transcript.

An implementation MUST NOT claim TLR/1 conformance on the basis of this prose
alone. The corpus is normative where the two disagree, and the corpus is machine
checkable.

## 2. Runs and envelopes

A **run** is one execution of an agent. It is identified by a `run_id` that MUST
match `[A-Za-z0-9._:-]{1,128}`.

A run has an envelope:

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `run_id` | string | yes | as above |
| `started_ts` | u64 | yes | milliseconds since the Unix epoch |
| `ended_ts` | u64 \| null | no | set when the run is sealed |
| `agent.name` | string | yes | the agent's stable identifier |
| `agent.version` | string | yes | the version that produced this run |
| `subject_ref` | string \| null | no | opaque reference to the decision's subject |
| `labels` | object of string→string | no | query dimensions: product, jurisdiction, model family |
| `head_seq` | u64 | yes | `seq` of the most recent event |
| `head_hash` | hex(32) | yes | `hash` of the most recent event |

`subject_ref` MUST NOT contain personal data. It is a pointer into the
implementer's own systems, present so a record can be located without the record
itself becoming a copy of the subject's file.

The envelope is not a separate authenticated object. It is carried as the
metadata of the run's first event (§3), and is therefore committed by the chain
like any other field. An implementation that stores the envelope outside the
chain permits an undetectable rewrite of which agent made a decision, and does
not conform.

## 3. Events

A run is an ordered sequence of events. `seq` MUST start at 0 and increase by
exactly 1; gaps and duplicates are invalid records, not recoverable states.

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `seq` | u64 | yes | server-assigned |
| `ts` | u64 | yes | server-assigned, milliseconds since the Unix epoch |
| `kind` | string | yes | one of §3.1 |
| `role` | string | no | e.g. `user`, `assistant`, `tool` |
| `name` | string | no | tool name, model name, decision name |
| `content` | string | no | the payload |
| `metadata` | object | no | structured detail; see §4.2 |
| `redacted` | object of string→hex(32) | no | retained digests of erased fields (§6) |
| `prev_hash` | hex(32) | yes | `hash` of event `seq - 1`, or 64 zeros at `seq` 0 |
| `hash` | hex(32) | yes | see §4 |

Servers MUST assign `seq`, `ts`, `prev_hash`, and `hash`. A client-supplied value
for any of them MUST be ignored. A record whose timestamps are chosen by the party
being audited is not evidence.

### 3.1 Kinds

| Kind | Meaning |
| --- | --- |
| `run_started` | Opens the run. MUST be `seq` 0; its metadata is the envelope. |
| `message` | A conversation message. |
| `model_call` | A model invocation. |
| `tool_call` | A tool or function call. |
| `decision` | A decision point, including approval requests and resolutions (§7). |
| `redaction` | Records an erasure (§6). |
| `run_finished` | Seals the run. MUST be terminal; carries `event_count` in metadata. |

An implementation encountering an unrecognised kind MUST reject the event. It MUST
NOT substitute a default, which would silently relabel the event.

## 4. The canonical form

### 4.1 Layout

Canon is exactly 181 bytes. All integers are big-endian.

| Offset | Length | Contents |
| --- | --- | --- |
| 0 | 5 | `74 6c 72 31 0a` — the ASCII bytes `tlr1\n` |
| 5 | 8 | `seq` as u64 |
| 13 | 8 | `ts` as u64 |
| 21 | 32 | SHA-256 of the `kind` wire string |
| 53 | 32 | digest of `role` |
| 85 | 32 | digest of `name` |
| 117 | 32 | digest of `content` |
| 149 | 32 | digest of `metadata` |

Each digest resolves by one rule:

```
digest(field) = SHA-256(utf8(value))   when the value is present
              = redacted[field]         when the value was erased (§6)
              = 32 × 0x00               when the value was never set
```

A present value MUST take precedence over a `redacted` entry for the same field.
Without this rule a forged `redacted` map could restate the hash of an event that
still carries its content.

An absent field and an empty string MUST produce different digests: `32 × 0x00`
and `SHA-256("")` respectively. One is the absence of a claim; the other is a
claim that the value is empty.

Committing every variable-length field by digest rather than by value keeps canon
fixed-width, which removes the framing rules where cross-language implementations
usually diverge, and makes §6 possible at all.

### 4.2 Metadata bytes

`metadata` MUST be hashed as the exact UTF-8 bytes the client sent. A server MUST
capture the byte span of the `metadata` member's value from the request body
before deserialising it, MUST store those bytes, and MUST return them unchanged.
It MUST NOT re-serialise.

JSON serialisers differ in key order, spacing, escaping, and number formatting.
Re-serialising would produce a different byte string and therefore a different
hash, breaking every verifier downstream. Vectors
`metadata-key-order-is-preserved` and `metadata-spacing-is-preserved` exist to
catch exactly this.

**Clients are bound by the same rule.** A verifier MUST read `metadata` from the
bytes of the response it received. Parsing a record into a generic JSON value and
then converting it to a typed structure re-serialises the metadata — most
languages' generic value types reorder object keys, and several renormalise
numbers — which changes the digest and fails a record that is perfectly sound.
Implementations MUST deserialize with a type that retains the original bytes
(`serde_json::value::RawValue` in Rust, `json.RawMessage` in Go) or read the
member's byte span from the response text directly (TypeScript, Python).

This is the most common way to get TLR/1 wrong, and it fails as "your record does
not verify", which is the worst diagnostic an audit trail can give. Corpus vector
`metadata-key-order-is-preserved` exists to catch it before anything reaches
production.

This protocol deliberately uses no JSON canonicalisation scheme. RFC 8785 is a
standard, but its number and Unicode rules are a source of cross-language
disagreement, and hashing what the client sent has the further merit of
preserving what the agent actually claimed. Key order is the shallowest part of
canonicalisation and the easiest to be tempted by; number formatting and Unicode
escaping are where cross-language schemes actually break.

### 4.3 The event hash

```
hash = SHA-256( canon ‖ prev_hash )
```

where `prev_hash` is 32 raw bytes. For `seq` 0, `prev_hash` MUST be 32 zero bytes.

## 5. The chain

A record verifies when, reading events in `seq` order:

1. the record is non-empty;
2. event 0 has kind `run_started`;
3. each `seq` equals its index;
4. each `prev_hash` equals the previous event's `hash`, and event 0's is zero;
5. each `hash` equals the value recomputed by §4.3.

This detects edited fields, inserted events, deleted events, and rewritten links,
and identifies the event at fault.

**It does not detect truncation of the tail.** Removing the last N events leaves a
valid prefix, which is a valid chain. Section 8 exists for this reason, and an
implementation MUST NOT claim that chain verification alone proves completeness.

## 6. Redaction

Article 12 of the EU AI Act requires the record be kept; Article 17 of the GDPR
requires personal data be erased on request. TLR/1 resolves the conflict by
erasing values while retaining what commits to them.

To erase fields of event `T`:

1. Append a `redaction` event whose metadata carries `target_seq`, the list of
   erased field names, and the authority for the erasure.
2. Move each erased field's digest into event `T`'s `redacted` map.
3. Discard the plaintext.

Event `T`'s canon, and therefore every subsequent hash, is unchanged: the digest
that was computed from the value is now supplied directly. A verifier holding only
the redacted record still validates the full chain, and the retained digest still
commits to what the erased value was.

Only `role`, `name`, `content`, and `metadata` are redactable. `seq`, `ts`, and
`kind` are structural: erasing them would destroy the record's shape rather than
its contents.

## 7. Approvals

An approval gate is a pair of events in the chain, not a separate resource. An
implementation storing approvals outside the chain permits the record and the
oversight trail to disagree, and does not conform.

**Request** — a `decision` event named `approval_requested`, metadata:

| Field | Type | Notes |
| --- | --- | --- |
| `action` | string | what the agent proposes to do |
| `expires_at` | u64 | when the gate lapses |

**Resolution** — a `decision` event named `approval_resolved`, metadata:

| Field | Type | Notes |
| --- | --- | --- |
| `target_seq` | u64 | the request's `seq` |
| `decision` | string | `approved`, `rejected`, or `expired` |
| `reviewer` | string \| null | the reviewer as claimed |
| `reviewer_principal` | string \| null | the authenticated identity, when the server knows one |
| `attested` | bool | whether the server authenticated the reviewer |
| `note` | string \| null | the reviewer's reasoning |

A gate's state is a fold over the chain: pending until a matching
`approval_resolved` appears. A server MUST reject a second resolution of the same
`target_seq`.

A lapsed gate MUST be resolved as `expired` and written into the chain. Nobody
answering is itself evidence, and a record that simply stops is indistinguishable
from one that was truncated.

`attested` MUST be `false` when the reviewer's identity is self-declared. An
implementation MUST NOT report `attested: true` on the basis of a caller-supplied
string. Honest weakness in the record is worth more than a claim the record
cannot support.

## 8. Checkpoints

A checkpoint is a signed assertion that a run's head was a given hash at a given
sequence. It closes the truncation gap in §5: anyone holding a checkpoint issued
after the deleted events can prove the deletion.

| Field | Type |
| --- | --- |
| `run_id` | string |
| `seq` | u64 |
| `head_hash` | hex(32) |
| `ts` | u64 |
| `key_id` | string |
| `sig` | base64 of a 64-byte Ed25519 signature over §8.1 |

### 8.1 Checkpoint canon

Exactly 119 bytes, following §4.1's convention:

| Offset | Length | Contents |
| --- | --- | --- |
| 0 | 7 | `tlrcp1\n` |
| 7 | 32 | SHA-256 of `run_id` |
| 39 | 8 | `seq` as u64 |
| 47 | 32 | `head_hash`, raw |
| 79 | 8 | `ts` as u64 |
| 87 | 32 | SHA-256 of `key_id` |

Servers SHOULD write a checkpoint at a configured interval and MUST write one when
a run is sealed. Public keys MUST be published at `/v1/.well-known/tideline` (§9).

Anchoring a checkpoint to an external timestamp authority is out of scope. The
format is designed to be anchorable, and an implementation MAY offer it as a
service on top.

## 9. HTTP binding

All routes are under `/v1`. Every response MUST carry `Tideline-Protocol: 1`.

| Method and path | Purpose |
| --- | --- |
| `GET /v1/.well-known/tideline` | capabilities, supported protocol versions, public keys |
| `POST /v1/runs` | create a run; writes `run_started` |
| `GET /v1/runs` | list runs, paginated, filtered by label, agent, and time |
| `GET /v1/runs/:id` | the envelope, head seq, head hash |
| `POST /v1/runs/:id/events` | append → `{seq, ts, hash, prev_hash}` |
| `GET /v1/runs/:id/events?from=&limit=` | events, paginated |
| `GET /v1/runs/:id/watch` | live tail over SSE |
| `POST /v1/runs/:id/complete` | seal the run; write `run_finished` and a checkpoint |
| `GET /v1/runs/:id/checkpoint` | the latest checkpoint |
| `POST /v1/runs/:id/approvals` | open a gate → `{seq}` |
| `GET /v1/runs/:id/approvals` | gates still pending |
| `GET /v1/runs/:id/approvals/:seq` | one gate's state |
| `POST /v1/runs/:id/approvals/:seq/resolve` | a human decides |
| `POST /v1/runs/:id/redactions` | erase fields, keep the chain (§6) |

### 9.1 Status codes

| Code | When |
| --- | --- |
| 200 | success |
| 400 | malformed body, invalid id, unrecognised kind |
| 401 | missing or invalid credentials |
| 403 | valid credentials, wrong tenant |
| 404 | no such run, event, or gate |
| 409 | a gate already resolved, or a run already sealed |
| 413 | body over the server's limit |
| 500 | storage failure |

### 9.2 Idempotency

Write requests MAY carry `Idempotency-Key`. A server receiving a key it has
already seen for that run MUST return the original response and MUST NOT append a
second event.

This matters more here than in an ordinary API. A client that retries after a
timeout would otherwise place a duplicate into evidence, permanently, and the
duplicate would verify — the chain proves a record was not altered after the
fact, not that it was correct when written.

### 9.3 Reads

Read routes MUST authenticate. A server MUST derive the tenant from the
authenticated credential and MUST NOT accept a tenant or namespace supplied as a
request parameter.

## 10. Versioning

The protocol version appears in the path (`/v1`) and in the `Tideline-Protocol`
response header. `/v1/.well-known/tideline` advertises supported versions,
capabilities, and signing keys, so a client can discover what a server offers
without trying routes.

Any change to §4 changes every hash and invalidates every record already written.
Such a change MUST take a new version number. Vectors in the corpus MUST NOT be
edited in place.

## 11. Security considerations

**The chain proves integrity, not truth.** It shows a record was not altered after
it was written. It says nothing about whether the agent reported its actions
honestly in the first place. Capture belongs in the execution path for this
reason.

**Truncation needs checkpoints.** §5 is blind to a removed tail. A deployment
without checkpoints, or without retaining them independently of the server that
issued them, has no truncation defence.

**Checkpoint keys are the trust root.** A server that can re-sign arbitrary
history can rewrite it. Keys SHOULD be held separately from the record store, and
checkpoints SHOULD be anchored externally where the threat model includes the
operator.

**Redaction is irreversible and auditable.** §6 keeps the digest, so an erased
value can be confirmed if it is supplied from elsewhere, but cannot be recovered
from the record.

**`subject_ref` and `labels` are not redactable** and appear in list queries. They
MUST NOT carry personal data.
