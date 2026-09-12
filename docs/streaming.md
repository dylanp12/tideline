# Streaming

Tideline's original job, and still the spine `GET /v1/runs/:id/watch` is built on:
deliver model output to clients reliably. Resume after a disconnect, fan one
generation out to many viewers, replay for late joiners, and keep a slow client
from stalling anyone else.

```bash
# your backend publishes chunks as the model generates
curl -XPOST localhost:8080/streams/demo --data 'Hello '
curl -XPOST localhost:8080/streams/demo --data 'world'
curl -XPOST localhost:8080/streams/demo/complete

# any client reads the whole stream over SSE
curl -N localhost:8080/streams/demo
# id: 0
# data: Hello
#
# id: 1
# data: world
#
# event: done

# reconnect and resume — Last-Event-ID skips what you already saw
curl -N -H 'Last-Event-ID: 0' localhost:8080/streams/demo
# id: 1
# data: world
# event: done
```

Because the offset travels in each `id:` field, browsers resume automatically on
reconnect with no client code at all.

```ts
import { useStream } from "@tideline/sdk/react";

function Answer({ id }) {
  const { text, status, reconnects } = useStream("http://localhost:8080", id);
  // status: connecting | streaming | reconnecting | done | error
  return <p data-status={status}>{text}</p>;
}
```

## API

| Method and path | Purpose |
| --- | --- |
| `POST /streams/:id` | append a chunk (body = the chunk) → returns the offset |
| `POST /streams/:id/complete` | finish successfully |
| `POST /streams/:id/error` | terminate with an error (body = the message) |
| `DELETE /streams/:id` | drop the stream |
| `GET /streams/:id` | subscribe over SSE (`Last-Event-ID` or `?from=` to resume) |
| `GET /streams/:id/ws` | subscribe over WebSocket (`?from=`, `?token=`) |

SSE event types: the default message (a chunk, carrying its offset in `id:`),
`gap`, `stream_error`, and `done`.

### Transports

Tideline serves the same stream over either transport:

- **SSE** (default) — browsers auto-resume via `Last-Event-ID`, no client code. Ideal for one-way token streaming.
- **WebSocket** (`/streams/:id/ws`) — one bidirectional, binary-capable socket; JSON frames `{ "type": "token" | "gap" | "error" | "done", … }`. For non-browser clients, or when you want a single socket you can also write to.

Both are thin encoders over one internal signal stream, so adding a WebTransport / HTTP-3 transport later is additive, not a rewrite. Reach for SSE unless you specifically need a bidirectional or non-browser/binary client.

### Scaling out

By default Tideline runs as a single in-memory instance. Set `TIDELINE_REDIS_URL` and it becomes a **fleet of stateless instances sharing one Redis** — publish to any instance, subscribe from any other, and resume / replay / fan-out still hold:

```bash
TIDELINE_REDIS_URL=redis://my-redis:6379 cargo run
```

Each stream is a Redis Stream (an append-only, MAXLEN-trimmed log); offsets are assigned atomically with a Lua script, subscribers replay with `XRANGE` and tail live with a blocking `XREAD`, so the resume seam stays exact across processes — no lost or duplicated token, no sticky sessions. Redis TTLs handle lifecycle, so no reaper runs. Verified end-to-end by cross-instance tests (publish on one backend, subscribe on another sharing the same Redis).\n
## Benchmarks

One generation fanned out to many concurrent SSE clients — single core,
in-process, 100% delivery:

| subscribers | tokens | events delivered | time | throughput |
| --- | --- | --- | --- | --- |
| 1,000 | 100 | 100,000 | 0.10 s | ~950k ev/s |
| 5,000 | 100 | 500,000 | 0.96 s | ~520k ev/s |
| 10,000 | 50 | 500,000 | 1.17 s | ~430k ev/s |

Reproduce with `cargo run --release -p tideline-server --example loadtest -- 10000 50`.
Loopback on one machine, so treat it as a floor rather than a cloud number.

## Scaling out

Set `TIDELINE_REDIS_URL` and the server becomes one of a fleet of stateless
instances sharing a Redis: publish to any, subscribe from any, and resume,
replay, and fan-out still hold.

```bash
TIDELINE_REDIS_URL=redis://my-redis:6379 \
TIDELINE_RECORD_DB=/data/tideline.db \
TIDELINE_ALLOW_LOCAL_RECORDS=1 \
  tideline-server
```

Each stream is a Redis Stream — an append-only, MAXLEN-trimmed log. Offsets are
assigned atomically with a Lua script, subscribers replay with `XRANGE` and tail
with a blocking `XREAD`, so the resume seam stays exact across processes: no
lost or duplicated chunk, and no sticky sessions. Redis TTLs handle lifecycle,
so no reaper runs.

Note that this shares *streams*, not *records*. Record storage is per-instance
until a shared store lands, which is why the server refuses to start with Redis
configured unless you confirm it is the only writer.
