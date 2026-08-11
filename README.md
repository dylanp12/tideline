# Tideline

**Durable, resumable real-time streams — a single small binary.**

Tideline is a stream engine for the output AI systems generate token by token:
publish chunks to a stream over HTTP, and any number of subscribers get them live
over SSE or WebSocket — with **free resume**. A subscriber that reconnects picks up
exactly where it left off (`Last-Event-ID`), a late subscriber replays from any
offset, and a producer crash is a recorded fact (`error`), not a hung connection.

```
┌──────────┐  POST /streams/:id   ┌──────────┐  SSE / WS (resumable)  ┌────────────┐
│ producer │ ───────────────────▶ │ tideline │ ─────────────────────▶ │ subscribers│
└──────────┘  complete / error    └──────────┘  replay from offset    └────────────┘
```

- **One binary, zero config** to start; in-memory engine with automatic stream reaping.
- **Horizontal scale when you want it:** set `TIDELINE_REDIS_URL` and every instance
  shares streams through Redis — resume and fan-out work across instances.
- **Auth that fits machines and browsers:** a bearer token for producers, and
  short-lived **HMAC tickets** for subscribers — mint them in your backend, hand them
  to a browser `EventSource`, no cookies or CORS gymnastics (CORS is permissive by design).
- **Multi-tenant namespacing** with per-tenant stream attribution events (optional,
  off by default).
- **Zero-dependency JS client** (`sdk/`) — browser `EventSource` handles reconnect + resume.

## Quickstart

```sh
cargo run --release
# tideline listening on http://0.0.0.0:8080  (publishing OPEN — dev mode)
```

Produce and consume:

```sh
curl -XPOST localhost:8080/streams/demo -d 'hello '
curl -XPOST localhost:8080/streams/demo -d 'world'
curl -XPOST localhost:8080/streams/demo/complete

curl -N 'localhost:8080/streams/demo?from=0'
# id: 0
# data: hello
# ...
```

Or from a browser:

```js
import { subscribe } from "./sdk/tideline.js";
subscribe("http://localhost:8080", "demo", {
  onToken: (data, offset) => out.append(data),
  from: 0,
});
```

## API

| Route | What it does |
|---|---|
| `POST /streams/:id` | Append a chunk (creates the stream on first write) |
| `GET /streams/:id?from=N` | Subscribe over SSE, replaying from offset N; `Last-Event-ID` resumes |
| `GET /streams/:id/ws` | The same, over WebSocket |
| `POST /streams/:id/complete` | Terminate the stream successfully |
| `POST /streams/:id/error` | Terminate with an error (subscribers see it — no silent hangs) |
| `DELETE /streams/:id` | Drop the stream and its buffer |
| `GET /health` · `GET /metrics` | Liveness + engine counters |

## Auth

Unset = open (dev). For production set either:

- `TIDELINE_PUBLISH_TOKEN` — static bearer token checked on every write, or
- `TIDELINE_AUTH_URL` — your backend verifies producer keys (Tideline calls it and
  caches the answer).

Subscribers use **tickets**: `base64url(payload).base64url(HMAC-SHA256(payload))`
where payload is `{"ns":"","sid":"<stream>","exp":<unix seconds>}`, signed with
`TIDELINE_AUTH_SECRET`. Mint them server-side per stream, hand them to the browser:

```
GET /streams/:id?from=0&ticket=<ticket>
```

A tampered or expired ticket is a 401. Streams can also be marked private so only
ticket-holders may read.

## Deploy

The included `Dockerfile` and `fly.toml` run it as a single always-on instance
(≈$3–4/month on Fly.io). For multiple instances, point them all at Redis.

## Who's behind this

Tideline is built and maintained by **[Paraph](https://paraphhq.vercel.app)** — it's
the real-time spine under Paraph's live human-oversight control room for AI agents.
It's released here as a standalone engine because resumable streams are useful far
beyond that product. Issues and PRs welcome.

MIT © 2026 Dylan Parent
