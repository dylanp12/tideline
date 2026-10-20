/**
 * Official TypeScript client for TLR/1 — the Tideline Record Protocol.
 *
 * ```ts
 * import { Tideline } from "@tideline/sdk";
 *
 * const tl  = new Tideline({ url: "http://localhost:8080" });
 * const run = await tl.startRun({
 *   agent: { name: "underwriter", version: "2.1.0" },
 *   runId: "loan-4821",
 *   subjectRef: "applicant-4821",
 * });
 *
 * await run.record({ kind: "tool_call", name: "pull_credit_report", content: "score=690" });
 *
 * // Waits until a person decides. Both halves are in the chain by then.
 * const decision = await run.gate({ action: "Approve EUR40,000 loan", expiresIn: 7200 });
 * if (decision.approved) {
 *   await run.record({ kind: "decision", name: "loan_approved", content: "EUR40,000" });
 * }
 * await run.complete();
 *
 * const { chain } = await run.verifiedEvents();
 * console.log(`${chain.len} events verified`);
 * ```
 *
 * `verifyChain` and `parseRecord` need no server and no network: an auditor can
 * check an exported record in a browser tab.
 */

export { Tideline, Run, TidelineError } from "./client.js";
export type {
  Appended,
  Decision,
  Gate,
  NewEvent,
  Resolution,
  TidelineOptions,
} from "./client.js";

export { verifyChain, ChainError } from "./chain.js";
export type { ChainOk, ChainFailure } from "./chain.js";

export { parseRecord, parseEvent } from "./event.js";
export type { TlrEvent, RunEnvelope, Checkpoint } from "./event.js";

export { canon, hashEvent, CANON_LEN } from "./canon.js";
export { rawMember, splitArray, ParseError } from "./parse.js";
export { sha256, sha256Utf8, toHex, fromHex, ZERO_HEX } from "./sha256.js";

export { publish, complete, fail, subscribe } from "./streams.js";
export type { StreamOptions, SubscribeHandlers } from "./streams.js";

export { EVENT_KINDS, REDACTABLE, isEventKind } from "./kinds.js";
export type { EventKind, RedactableField } from "./kinds.js";
