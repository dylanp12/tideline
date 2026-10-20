# Demo

`tideline.gif` (used in the root README) is a **real capture** of the engine, not a
mockup: tokens stream live over SSE, the connection drops at an offset, and a
reconnect with `Last-Event-ID` resumes exactly — no gap, no repeat.

Regenerate it:

```bash
./record.sh   # needs: cargo, asciinema, agg, a free :8080
```

- `demo.sh` — the scripted narrative (paced publisher + a subscriber that drops and resumes).
- `record.sh` — boots the engine, records `demo.sh` with [asciinema](https://asciinema.org),
  and renders the GIF with [agg](https://github.com/asciinema/agg).
