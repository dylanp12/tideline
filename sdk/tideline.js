// Tideline JS SDK — zero dependencies.
//
// Browser: publish / complete / fail via fetch; subscribe via the native
// EventSource, which auto-reconnects on a dropped connection and resumes from the
// last offset via Last-Event-ID — so resume is free, no client code.

const auth = (token) => (token ? { authorization: `Bearer ${token}` } : undefined);

/** Append one chunk to a stream (call as your model generates tokens). */
export async function publish(baseUrl, streamId, data, { token } = {}) {
  await fetch(`${baseUrl}/streams/${encodeURIComponent(streamId)}`, {
    method: "POST",
    headers: auth(token),
    body: data,
  });
}

/** Mark a stream finished successfully. */
export async function complete(baseUrl, streamId, { token } = {}) {
  await fetch(`${baseUrl}/streams/${encodeURIComponent(streamId)}/complete`, {
    method: "POST",
    headers: auth(token),
  });
}

/** Terminate a stream with an error message (terminal). */
export async function fail(baseUrl, streamId, message = "", { token } = {}) {
  await fetch(`${baseUrl}/streams/${encodeURIComponent(streamId)}/error`, {
    method: "POST",
    headers: auth(token),
    body: message,
  });
}

/**
 * Subscribe to a stream. Callbacks:
 *  - onToken(data, offset)   per chunk
 *  - onDone()                stream completed successfully (terminal)
 *  - onStreamError(message)  producer ended the stream with an error (terminal)
 *  - onGap()                 a resume point was evicted (discontinuity)
 *  - onReconnect()           the connection dropped; EventSource is retrying
 *  - onOpen()                (re)connected
 *  - from                    explicit start/resume offset (manual reconnects)
 *  - namespace               your Tideline Cloud account id (open reads)
 *  - ticket                  signed ticket to read a *private* stream
 *                            (mint server-side via POST /api/v1/subscribe-token)
 * Returns the EventSource; call .close() to stop.
 */
export function subscribe(baseUrl, streamId, opts = {}) {
  const { from, namespace, ticket, onToken, onDone, onStreamError, onGap, onReconnect, onOpen } = opts;
  const qs = new URLSearchParams();
  if (from != null) qs.set("from", String(from));
  // A ticket carries its own namespace, so it supersedes `ns`.
  if (ticket) qs.set("ticket", ticket);
  else if (namespace) qs.set("ns", namespace); // Tideline Cloud: your account/namespace id
  const url = `${baseUrl}/streams/${encodeURIComponent(streamId)}${qs.toString() ? `?${qs}` : ""}`;
  const es = new EventSource(url);
  let opened = false;

  es.onopen = () => {
    opened = true;
    onOpen && onOpen();
  };
  es.onmessage = (e) => onToken && onToken(e.data, Number(e.lastEventId));
  es.addEventListener("gap", () => onGap && onGap());
  es.addEventListener("stream_error", (e) => {
    es.close();
    onStreamError && onStreamError(e.data || "");
  });
  es.addEventListener("done", () => {
    es.close();
    onDone && onDone();
  });
  // Native EventSource "error" = a transport problem; it auto-retries, resuming
  // via Last-Event-ID. We surface that as a reconnect, not a stream failure.
  es.onerror = () => {
    if (opened) {
      opened = false;
      onReconnect && onReconnect();
    }
  };
  return es;
}

/**
 * Subscribe over WebSocket instead of SSE — one bidirectional socket. Same
 * callbacks as `subscribe`. Unlike SSE (where the browser auto-resumes), resume
 * is handled here: we track the last offset and reconnect with `?from=`.
 *  - token  passed as `?token=` (browsers can't set WS headers)
 *  - from   explicit start/resume offset (default 0)
 * Returns a handle with `.close()`.
 */
export function subscribeWs(baseUrl, streamId, opts = {}) {
  const { from = 0, namespace, ticket, token, onToken, onDone, onStreamError, onGap, onReconnect, onOpen } = opts;
  const wsBase = baseUrl.replace(/^http/, "ws");
  let next = from; // next offset we expect (for resume)
  let closed = false;
  let backoff = 250;
  let ws;

  const connect = () => {
    let url = `${wsBase}/streams/${encodeURIComponent(streamId)}/ws?from=${next}`;
    if (ticket) url += `&ticket=${encodeURIComponent(ticket)}`; // private stream
    else if (namespace) url += `&ns=${encodeURIComponent(namespace)}`;
    if (token) url += `&token=${encodeURIComponent(token)}`;
    ws = new WebSocket(url);
    ws.onopen = () => {
      backoff = 250;
      onOpen && onOpen();
    };
    ws.onmessage = (e) => {
      let m;
      try {
        m = JSON.parse(e.data);
      } catch {
        return;
      }
      switch (m.type) {
        case "token":
          next = m.offset + 1;
          onToken && onToken(m.data, m.offset);
          break;
        case "gap":
          onGap && onGap();
          break;
        case "error":
          closed = true;
          try { ws.close(); } catch {}
          onStreamError && onStreamError(m.message || "");
          break;
        case "done":
          closed = true;
          try { ws.close(); } catch {}
          onDone && onDone();
          break;
      }
    };
    ws.onclose = () => {
      if (closed) return;
      onReconnect && onReconnect();
      setTimeout(connect, backoff);
      backoff = Math.min(backoff * 2, 5000); // resume from `next` on reconnect
    };
    ws.onerror = () => {
      try { ws.close(); } catch {}
    };
  };

  connect();
  return { close: () => { closed = true; try { ws.close(); } catch {} } };
}
