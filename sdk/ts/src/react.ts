/**
 * React hooks.
 *
 * `useStream` follows transient model output. `useRun` and `useApprovalQueue`
 * follow the durable record — the two things a live oversight view shows side
 * by side: what the agent is saying, and what it is waiting for.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { Tideline, type Gate } from "./client.js";
import type { TlrEvent } from "./event.js";
import { subscribe, type StreamOptions } from "./streams.js";

export type StreamStatus = "connecting" | "streaming" | "reconnecting" | "done" | "error";

/**
 * Follow a token stream, with resume handled by the browser.
 *
 * `reconnects` is surfaced deliberately: a stream that silently reconnects
 * twelve times is a different operational story from one that never does.
 */
export function useStream(baseUrl: string, streamId: string | null, options: StreamOptions = {}) {
  const [text, setText] = useState("");
  const [status, setStatus] = useState<StreamStatus>("connecting");
  const [reconnects, setReconnects] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const { namespace, ticket } = options;

  useEffect(() => {
    if (!streamId) return;
    setText("");
    setStatus("connecting");
    setReconnects(0);
    setError(null);

    const es = subscribe(baseUrl, streamId, {
      namespace,
      ticket,
      onOpen: () => setStatus((s) => (s === "done" || s === "error" ? s : "streaming")),
      onToken: (data) => {
        setText((t) => t + data);
        setStatus("streaming");
      },
      onReconnect: () => {
        setReconnects((n) => n + 1);
        setStatus("reconnecting");
      },
      onStreamError: (msg) => {
        setError(msg);
        setStatus("error");
      },
      onDone: () => setStatus("done"),
    });
    return () => es.close();
  }, [baseUrl, streamId, namespace, ticket]);

  return { text, status, reconnects, error };
}

/**
 * Follow a run's record live.
 *
 * `verified` is not a decoration. A viewer that shows a record without saying
 * whether it verifies is a log viewer, and the difference between a log and
 * evidence is exactly that check.
 */
export function useRun(client: Tideline, runId: string | null) {
  const [events, setEvents] = useState<TlrEvent[]>([]);
  const [verified, setVerified] = useState<boolean | null>(null);
  const [error, setError] = useState<Error | null>(null);
  const abort = useRef<AbortController | null>(null);

  useEffect(() => {
    if (!runId) return;
    setEvents([]);
    setVerified(null);
    setError(null);

    const controller = new AbortController();
    abort.current = controller;
    const run = client.run(runId);

    void (async () => {
      try {
        for await (const e of run.watch({ signal: controller.signal })) {
          setEvents((prev) => [...prev, e]);
        }
      } catch (e) {
        if (!controller.signal.aborted) setError(e as Error);
      }
    })();

    return () => controller.abort();
  }, [client, runId]);

  const verify = useCallback(async () => {
    if (!runId) return;
    try {
      const { chain } = await client.run(runId).verifiedEvents();
      setVerified(true);
      return chain;
    } catch (e) {
      setVerified(false);
      setError(e as Error);
      return undefined;
    }
  }, [client, runId]);

  return { events, verified, error, verify };
}

/**
 * A reviewer's queue for one run, polled.
 *
 * Polling on purpose: a queue that a person is watching wants a predictable
 * refresh, and a gate may sit for hours — longer than any connection a proxy
 * will hold open.
 */
export function useApprovalQueue(client: Tideline, runId: string | null, intervalMs = 3000) {
  const [gates, setGates] = useState<Gate[]>([]);
  const [error, setError] = useState<Error | null>(null);

  const refresh = useCallback(async () => {
    if (!runId) return;
    try {
      setGates(await client.run(runId).approvals());
      setError(null);
    } catch (e) {
      setError(e as Error);
    }
  }, [client, runId]);

  useEffect(() => {
    if (!runId) return;
    void refresh();
    const id = setInterval(() => void refresh(), intervalMs);
    return () => clearInterval(id);
  }, [runId, intervalMs, refresh]);

  const resolve = useCallback(
    async (seq: number, decision: "approved" | "rejected", reviewer?: string, note?: string) => {
      if (!runId) return;
      await client.run(runId).resolve(seq, { decision, reviewer, note });
      await refresh();
    },
    [client, runId, refresh],
  );

  return { gates, error, refresh, resolve };
}
