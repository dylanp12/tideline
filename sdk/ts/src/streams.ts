/**
 * The token-streaming client.
 *
 * Separate from the record: a stream is transient output on its way to a user,
 * a record is durable evidence. Both run on the same engine, and an app that
 * streams an answer to a browser while recording the run behind it uses both.
 */

export interface StreamOptions {
  token?: string;
  /** Tenant namespace (Cloud). Non-secret: the publisher's account id. */
  namespace?: string;
  /** Signed ticket for a private stream, minted server-side. */
  ticket?: string;
}

const auth = (token?: string) => (token ? { authorization: `Bearer ${token}` } : undefined);

/** Append one chunk, as the model generates it. */
export async function publish(
  baseUrl: string,
  streamId: string,
  data: string,
  opts: StreamOptions = {},
): Promise<void> {
  await fetch(`${baseUrl}/streams/${encodeURIComponent(streamId)}`, {
    method: "POST",
    headers: auth(opts.token),
    body: data,
  });
}

/** Mark a stream finished successfully. */
export async function complete(
  baseUrl: string,
  streamId: string,
  opts: StreamOptions = {},
): Promise<void> {
  await fetch(`${baseUrl}/streams/${encodeURIComponent(streamId)}/complete`, {
    method: "POST",
    headers: auth(opts.token),
  });
}

/** Terminate a stream with an error message. Terminal. */
export async function fail(
  baseUrl: string,
  streamId: string,
  message = "",
  opts: StreamOptions = {},
): Promise<void> {
  await fetch(`${baseUrl}/streams/${encodeURIComponent(streamId)}/error`, {
    method: "POST",
    headers: auth(opts.token),
    body: message,
  });
}

export interface SubscribeHandlers {
  onToken?: (data: string, offset: number) => void;
  onDone?: () => void;
  onStreamError?: (message: string) => void;
  /** A resume point had already been evicted; expect a discontinuity. */
  onGap?: () => void;
  onReconnect?: () => void;
  onOpen?: () => void;
  from?: number;
}

/**
 * Subscribe over SSE.
 *
 * Resume is free: the browser's EventSource resends `Last-Event-ID` on a
 * dropped connection and the server continues from that offset, so a network
 * blip mid-generation costs nothing and needs no client code.
 */
export function subscribe(
  baseUrl: string,
  streamId: string,
  opts: SubscribeHandlers & StreamOptions = {},
): EventSource {
  const qs = new URLSearchParams();
  if (opts.from != null) qs.set("from", String(opts.from));
  // A ticket carries its own namespace, so it supersedes `ns`.
  if (opts.ticket) qs.set("ticket", opts.ticket);
  else if (opts.namespace) qs.set("ns", opts.namespace);

  const suffix = qs.toString() ? `?${qs}` : "";
  const es = new EventSource(`${baseUrl}/streams/${encodeURIComponent(streamId)}${suffix}`);
  let opened = false;

  es.onopen = () => {
    opened = true;
    opts.onOpen?.();
  };
  es.onmessage = (e) => opts.onToken?.(e.data, Number(e.lastEventId));
  es.addEventListener("gap", () => opts.onGap?.());
  es.addEventListener("stream_error", (e) => {
    es.close();
    opts.onStreamError?.((e as MessageEvent).data ?? "");
  });
  es.addEventListener("done", () => {
    es.close();
    opts.onDone?.();
  });
  // A native EventSource "error" is a transport problem; it retries by itself,
  // resuming from Last-Event-ID. That is a reconnect, not a stream failure.
  es.onerror = () => {
    if (opened) {
      opened = false;
      opts.onReconnect?.();
    }
  };
  return es;
}
