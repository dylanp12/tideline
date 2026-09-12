import { type Checkpoint, type RunEnvelope, type TlrEvent, parseRecord } from "./event.js";
import { verifyChain, type ChainOk } from "./chain.js";
import type { EventKind, RedactableField } from "./kinds.js";

/** The server answered, and said no. */
export class TidelineError extends Error {
  constructor(
    readonly status: number,
    readonly body: string,
    message?: string,
  ) {
    super(message ?? `server returned ${status}: ${body}`);
    this.name = "TidelineError";
  }
  /** The record's state forbids this: a sealed run, a gate already decided. */
  get isConflict(): boolean {
    return this.status === 409;
  }
  get isNotFound(): boolean {
    return this.status === 404;
  }
  get isUnauthorized(): boolean {
    return this.status === 401;
  }
}

export interface TidelineOptions {
  url: string;
  /** Sent as a bearer token. */
  key?: string;
  /** Override for tests or a custom agent. */
  fetch?: typeof globalThis.fetch;
}

export interface NewEvent {
  kind: EventKind;
  role?: string;
  name?: string;
  content?: string;
  metadata?: unknown;
}

export interface Appended {
  seq: number;
  ts: number;
  prev_hash: string;
  hash: string;
}

export type Decision = "approved" | "rejected" | "expired";

export interface Gate {
  seq: number;
  action: string;
  expires_at: number;
  state: "pending" | "resolved";
  decision?: Decision;
  reviewer?: string;
  reviewer_principal?: string;
  attested: boolean;
  note?: string;
}

export interface Resolution {
  seq: number;
  decision: Decision;
  reviewer?: string;
  note?: string;
  /** Whether the server authenticated the reviewer, rather than taking their
   *  word for who they are. */
  attested: boolean;
  approved: boolean;
}

export class Tideline {
  private readonly base: string;
  private readonly key: string | undefined;
  private readonly doFetch: typeof globalThis.fetch;

  constructor(options: TidelineOptions | string) {
    const o = typeof options === "string" ? { url: options } : options;
    this.base = o.url.replace(/\/+$/, "");
    this.key = o.key;
    this.doFetch = o.fetch ?? globalThis.fetch.bind(globalThis);
  }

  /** @internal */
  async request(path: string, init: RequestInit = {}): Promise<Response> {
    const headers = new Headers(init.headers);
    if (this.key) headers.set("authorization", `Bearer ${this.key}`);
    if (init.body !== undefined) headers.set("content-type", "application/json");
    const res = await this.doFetch(`${this.base}${path}`, { ...init, headers });
    if (!res.ok) throw new TidelineError(res.status, await res.text().catch(() => ""));
    return res;
  }

  /** @internal Read a JSON response for things that are not records. */
  async json<T>(path: string, init?: RequestInit): Promise<T> {
    return (await this.request(path, init)).json() as Promise<T>;
  }

  /**
   * @internal Read a record.
   *
   * From the response text, never `res.json()`. Going through a parsed value
   * discards the metadata bytes, and every event then fails verification.
   */
  async record(path: string, init?: RequestInit): Promise<TlrEvent[]> {
    return parseRecord(await (await this.request(path, init)).text());
  }

  /** Open a run. Its envelope becomes the first link in the chain. */
  async startRun(opts: {
    runId: string;
    agent: { name: string; version: string };
    /** An opaque reference to the subject. Never personal data: it is not
     *  redactable and it appears in list queries. */
    subjectRef?: string;
    labels?: Record<string, string>;
  }): Promise<Run> {
    const env = await this.json<RunEnvelope>("/v1/runs", {
      method: "POST",
      body: JSON.stringify({
        run_id: opts.runId,
        agent: opts.agent,
        subject_ref: opts.subjectRef,
        labels: opts.labels,
      }),
    });
    return new Run(this, env.run_id);
  }

  /** A handle to a run that already exists. Contacts nothing. */
  run(runId: string): Run {
    return new Run(this, runId);
  }

  async listRuns(
    query: { agent?: string; label?: string; since?: number; cursor?: string; limit?: number } = {},
  ): Promise<{ runs: RunEnvelope[]; next_cursor?: string }> {
    const qs = new URLSearchParams();
    for (const [k, v] of Object.entries(query)) if (v !== undefined) qs.set(k, String(v));
    const suffix = qs.toString() ? `?${qs}` : "";
    return this.json(`/v1/runs${suffix}`);
  }

  /** Capabilities, supported protocol versions, and checkpoint signing keys. */
  async wellKnown(): Promise<{
    protocol_versions: number[];
    capabilities: string[];
    keys: { id: string; alg: string; public_key: string }[];
  }> {
    return this.json("/v1/.well-known/tideline");
  }
}

export class Run {
  constructor(
    private readonly client: Tideline,
    readonly id: string,
  ) {}

  private path(suffix = ""): string {
    return `/v1/runs/${encodeURIComponent(this.id)}${suffix}`;
  }

  async envelope(): Promise<RunEnvelope> {
    return this.client.json(this.path());
  }

  /**
   * Append an event.
   *
   * Pass `idempotencyKey` on anything you might retry: without one, a retry
   * after a timeout puts a duplicate into evidence permanently, and it verifies
   * — the chain proves a record was not altered afterwards, not that it was
   * right when written.
   */
  async record(event: NewEvent, opts: { idempotencyKey?: string } = {}): Promise<Appended> {
    return this.client.json(this.path("/events"), {
      method: "POST",
      body: JSON.stringify(event),
      headers: opts.idempotencyKey ? { "idempotency-key": opts.idempotencyKey } : undefined,
    });
  }

  /** Open a gate without waiting on it. */
  async openGate(opts: { action: string; expiresIn?: number }): Promise<{ seq: number; expires_at: number }> {
    return this.client.json(this.path("/approvals"), {
      method: "POST",
      body: JSON.stringify({ action: opts.action, expires_in: opts.expiresIn }),
    });
  }

  /**
   * Open a gate and wait until a person decides, or it expires.
   *
   * This is what puts oversight in the agent's path rather than beside it: when
   * it resolves, the request and the decision are both already in the chain.
   *
   * It polls rather than holding a stream open. A gate can sit for hours, and a
   * poll survives a proxy idle timeout, a NAT rebind, and a process restart
   * where a long-lived connection quietly does not. Use `watch` for a live
   * oversight view; use this for a gate.
   *
   * A rejection resolves rather than throwing: an agent must be able to branch
   * on a refusal without wrapping the normal case in error handling.
   */
  async gate(opts: { action: string; expiresIn?: number }): Promise<Resolution> {
    const { seq } = await this.openGate(opts);
    return this.awaitGate(seq);
  }

  async awaitGate(seq: number, opts: { signal?: AbortSignal } = {}): Promise<Resolution> {
    let delay = 250;
    for (;;) {
      if (opts.signal?.aborted) throw new Error("aborted while waiting on a gate");
      const gate = await this.approval(seq);
      if (gate.state !== "pending") {
        const decision = gate.decision;
        if (!decision) throw new Error(`gate ${seq} is resolved but carries no decision`);
        return {
          seq,
          decision,
          reviewer: gate.reviewer,
          note: gate.note,
          attested: gate.attested,
          approved: decision === "approved",
        };
      }
      await new Promise((r) => setTimeout(r, delay));
      // Back off to five seconds: nobody answers faster than that, and a tight
      // loop on a multi-hour gate is rude.
      delay = Math.min(delay * 2, 5000);
    }
  }

  /** Gates still awaiting a decision — the reviewer's queue. */
  async approvals(): Promise<Gate[]> {
    return this.client.json(this.path("/approvals"));
  }

  async approval(seq: number): Promise<Gate> {
    return this.client.json(this.path(`/approvals/${seq}`));
  }

  /** Record a person's decision on a gate. */
  async resolve(
    seq: number,
    opts: { decision: Exclude<Decision, "expired">; reviewer?: string; note?: string },
  ): Promise<void> {
    await this.client.json(this.path(`/approvals/${seq}/resolve`), {
      method: "POST",
      body: JSON.stringify(opts),
    });
  }

  async events(opts: { from?: number; limit?: number } = {}): Promise<TlrEvent[]> {
    const qs = new URLSearchParams({
      from: String(opts.from ?? 0),
      limit: String(opts.limit ?? 5000),
    });
    return this.client.record(this.path(`/events?${qs}`));
  }

  /** Fetch the record and verify it in one step. */
  async verifiedEvents(): Promise<{ events: TlrEvent[]; chain: ChainOk }> {
    const events = await this.events();
    return { events, chain: verifyChain(events) };
  }

  /** Follow the record live: history first, then events as they land. */
  async *watch(opts: { from?: number; signal?: AbortSignal } = {}): AsyncGenerator<TlrEvent> {
    const res = await this.client.request(this.path(`/watch?from=${opts.from ?? 0}`), {
      signal: opts.signal,
      headers: { accept: "text/event-stream" },
    });
    const body = res.body;
    if (!body) throw new Error("watch: the response carried no stream");

    const reader = body.getReader();
    const decoder = new TextDecoder();
    let buf = "";
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) return;
        buf += decoder.decode(value, { stream: true });
        let sep: number;
        while ((sep = buf.indexOf("\n\n")) !== -1) {
          const frame = buf.slice(0, sep);
          buf = buf.slice(sep + 2);
          let data = "";
          let terminal = false;
          for (const line of frame.split("\n")) {
            if (line.startsWith("data:")) data += line.slice(5).trimStart();
            else if (line.startsWith("event:")) {
              terminal = ["done", "stream_error"].includes(line.slice(6).trim());
            }
          }
          if (terminal) return;
          // From the frame text, so the metadata bytes survive.
          if (data) yield (await import("./event.js")).parseEvent(data);
        }
      }
    } finally {
      reader.releaseLock();
    }
  }

  async complete(): Promise<Appended> {
    return this.client.json(this.path("/complete"), { method: "POST" });
  }

  /** The latest signed checkpoint, or null if the server has issued none. */
  async checkpoint(): Promise<Checkpoint | null> {
    try {
      return await this.client.json<Checkpoint>(this.path("/checkpoint"));
    } catch (e) {
      if (e instanceof TidelineError && e.isNotFound) return null;
      throw e;
    }
  }

  /** Erase fields of an earlier event, keeping the chain intact. */
  async redact(targetSeq: number, fields: RedactableField[], authority: string): Promise<Appended> {
    return this.client.json(this.path("/redactions"), {
      method: "POST",
      body: JSON.stringify({ target_seq: targetSeq, fields, authority }),
    });
  }
}
