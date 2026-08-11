// Tideline React hook — `useStream`. Requires React 18+.
//
//   const { text, status, reconnects, error } = useStream(baseUrl, streamId);
//   // status: "connecting" | "streaming" | "reconnecting" | "done" | "error"
//
// Reconnect + resume are handled automatically by the browser's EventSource
// (it resends Last-Event-ID), so a dropped connection mid-generation just
// continues — surfaced here as `status: "reconnecting"` and a `reconnects` count.
import { useEffect, useRef, useState } from "react";
import { subscribe } from "./tideline.js";

export function useStream(baseUrl, streamId, options = {}) {
  const [text, setText] = useState("");
  const [status, setStatus] = useState("connecting");
  const [reconnects, setReconnects] = useState(0);
  const [error, setError] = useState(null);
  const esRef = useRef(null);

  useEffect(() => {
    if (!streamId) return;
    setText("");
    setStatus("connecting");
    setReconnects(0);
    setError(null);
    const es = subscribe(baseUrl, streamId, {
      namespace: options.namespace, // Tideline Cloud namespace (your account id)
      ticket: options.ticket, // signed ticket for a private stream (optional)
      onOpen: () => setStatus((s) => (s === "done" || s === "error" ? s : "streaming")),
      onToken: (data) => {
        setText((t) => t + data);
        setStatus("streaming");
      },
      onReconnect: () => {
        setReconnects((n) => n + 1);
        setStatus("reconnecting");
      },
      onGap: () => {}, // resumed past eviction; tokens continue from the tail
      onStreamError: (msg) => {
        setError(msg);
        setStatus("error");
      },
      onDone: () => setStatus("done"),
    });
    esRef.current = es;
    return () => es.close();
  }, [baseUrl, streamId, options.namespace, options.ticket]);

  return { text, status, reconnects, error };
}
