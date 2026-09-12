import type { TlrEvent } from "./event.js";
import { hashEvent } from "./canon.js";
import { ZERO_HEX } from "./sha256.js";

export type ChainFailure =
  | "empty"
  | "not_run_started"
  | "seq_gap"
  | "prev_mismatch"
  | "hash_mismatch";

/** A record that does not verify, and where. */
export class ChainError extends Error {
  constructor(
    readonly reason: ChainFailure,
    readonly seq: number | null,
    message: string,
  ) {
    super(message);
    this.name = "ChainError";
  }
}

export interface ChainOk {
  len: number;
  headSeq: number;
  headHash: string;
  /** True when the record ends in `run_finished`. */
  sealed: boolean;
}

/**
 * Verify an ordered run record end to end.
 *
 * Detects edited fields, inserted events, deleted events, and rewritten links,
 * and names the event at fault. It cannot detect truncation of the tail — a
 * valid prefix is a valid chain — which is what checkpoints are for. Check
 * `sealed`, and a checkpoint, before calling a record complete.
 */
export function verifyChain(events: TlrEvent[]): ChainOk {
  const first = events[0];
  if (!first) throw new ChainError("empty", null, "the record is empty");
  if (first.kind !== "run_started") {
    throw new ChainError(
      "not_run_started",
      0,
      `a record must open with run_started, got ${first.kind}`,
    );
  }

  let prev = ZERO_HEX;
  for (let i = 0; i < events.length; i++) {
    const e = events[i]!;
    if (e.seq !== i) {
      throw new ChainError("seq_gap", i, `expected seq ${i}, found ${e.seq}`);
    }
    if (e.prevHash !== prev) {
      throw new ChainError(
        "prev_mismatch",
        e.seq,
        `event ${e.seq} points at ${e.prevHash.slice(0, 16)}…, expected ${prev.slice(0, 16)}…`,
      );
    }
    const computed = hashEvent(e, prev);
    if (computed !== e.hash) {
      throw new ChainError(
        "hash_mismatch",
        e.seq,
        `event ${e.seq} was altered: it hashes to ${computed.slice(0, 16)}…, ` +
          `but claims ${e.hash.slice(0, 16)}…`,
      );
    }
    prev = computed;
  }

  const last = events[events.length - 1]!;
  return {
    len: events.length,
    headSeq: last.seq,
    headHash: last.hash,
    sealed: last.kind === "run_finished",
  };
}
