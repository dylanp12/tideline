/** The kinds of a TLR/1 event. The wire strings are hashed into canon. */
export const EVENT_KINDS = [
  "run_started",
  "message",
  "model_call",
  "tool_call",
  "decision",
  "redaction",
  "run_finished",
] as const;

export type EventKind = (typeof EVENT_KINDS)[number];

export function isEventKind(s: string): s is EventKind {
  return (EVENT_KINDS as readonly string[]).includes(s);
}

/** The fields a redaction may erase. `seq`, `ts`, and `kind` are structural. */
export const REDACTABLE = ["role", "name", "content", "metadata"] as const;
export type RedactableField = (typeof REDACTABLE)[number];
