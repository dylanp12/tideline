import { type EventKind, isEventKind } from "./kinds.js";
import { ParseError, rawMember, splitArray } from "./parse.js";

/**
 * One event on the record.
 *
 * `metadata` is the parsed value, for reading. `metadataRaw` is the exact text
 * the server sent, and is what canon hashes. Never rebuild one from the other.
 */
export interface TlrEvent {
  seq: number;
  ts: number;
  kind: EventKind;
  role?: string;
  name?: string;
  content?: string;
  metadata?: unknown;
  metadataRaw?: string;
  /** Retained digests of erased fields, keyed by field name. */
  redacted?: Record<string, string>;
  prevHash: string;
  hash: string;
}

export interface RunEnvelope {
  run_id: string;
  started_ts: number;
  ended_ts?: number | null;
  agent: { name: string; version: string };
  subject_ref?: string | null;
  labels?: Record<string, string>;
  head_seq: number;
  head_hash: string;
}

export interface Checkpoint {
  run_id: string;
  seq: number;
  head_hash: string;
  ts: number;
  key_id: string;
  sig: string;
}

/** Parse one event from its source text, keeping the metadata bytes. */
export function parseEvent(text: string): TlrEvent {
  const raw = JSON.parse(text) as Record<string, unknown>;
  const kind = raw["kind"];
  if (typeof kind !== "string" || !isEventKind(kind)) {
    // Defaulting would silently relabel the event.
    throw new ParseError(`unknown event kind ${JSON.stringify(kind)}`, 0);
  }
  const metadataRaw = rawMember(text, "metadata");
  return {
    seq: raw["seq"] as number,
    ts: raw["ts"] as number,
    kind,
    role: raw["role"] as string | undefined,
    name: raw["name"] as string | undefined,
    content: raw["content"] as string | undefined,
    metadata: raw["metadata"],
    // A null metadata member is absent, not an empty claim.
    metadataRaw: metadataRaw === "null" ? undefined : metadataRaw,
    redacted: raw["redacted"] as Record<string, string> | undefined,
    prevHash: (raw["prev_hash"] as string) ?? "0".repeat(64),
    hash: (raw["hash"] as string) ?? "0".repeat(64),
  };
}

/**
 * Parse a whole record from the response text.
 *
 * Always use this on a record. `JSON.parse` alone discards the metadata bytes
 * and every event will then fail verification.
 */
export function parseRecord(text: string): TlrEvent[] {
  return splitArray(text).map(parseEvent);
}
