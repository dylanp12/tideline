import type { TlrEvent } from "./event.js";
import { fromHex, sha256, sha256Utf8, toHex, ZERO } from "./sha256.js";

/** A 5-byte domain tag, two big-endian u64s, and five 32-byte field digests. */
export const CANON_LEN = 181;

const DOMAIN = new Uint8Array([0x74, 0x6c, 0x72, 0x31, 0x0a]); // "tlr1\n"

function writeU64BE(view: DataView, offset: number, value: number): void {
  // JavaScript numbers are exact to 2^53; a millisecond timestamp is ~2^41, and
  // a sequence number will not approach it either.
  view.setUint32(offset, Math.floor(value / 0x100000000));
  view.setUint32(offset + 4, value % 0x100000000);
}

/**
 * Resolve one field to its digest:
 * present → SHA-256 of the value; erased → the retained digest;
 * never set → 32 zero bytes.
 *
 * A present value always wins, so a forged `redacted` map cannot restate the
 * hash of an event that still carries its content.
 */
function fieldDigest(event: TlrEvent, field: string, value: string | undefined): Uint8Array {
  if (value !== undefined) return sha256Utf8(value);
  const retained = event.redacted?.[field];
  if (retained) return fromHex(retained);
  return ZERO;
}

/** The canonical 181-byte encoding of an event. */
export function canon(event: TlrEvent): Uint8Array {
  const out = new Uint8Array(CANON_LEN);
  const view = new DataView(out.buffer);

  out.set(DOMAIN, 0);
  writeU64BE(view, 5, event.seq);
  writeU64BE(view, 13, event.ts);
  out.set(sha256Utf8(event.kind), 21);
  out.set(fieldDigest(event, "role", event.role), 53);
  out.set(fieldDigest(event, "name", event.name), 85);
  out.set(fieldDigest(event, "content", event.content), 117);
  out.set(fieldDigest(event, "metadata", event.metadataRaw), 149);

  return out;
}

/** An event's hash, committing to its predecessor. */
export function hashEvent(event: TlrEvent, prevHashHex: string): string {
  const c = canon(event);
  const joined = new Uint8Array(CANON_LEN + 32);
  joined.set(c, 0);
  joined.set(fromHex(prevHashHex), CANON_LEN);
  return toHex(sha256(joined));
}
