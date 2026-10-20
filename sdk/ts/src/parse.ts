/**
 * A JSON reader that keeps the bytes.
 *
 * Canon commits to the exact UTF-8 of an event's `metadata` (`spec/tlr-1.md`
 * §4.2). `JSON.parse` throws that away: what comes back is a value, and
 * re-serialising it reorders keys and renormalises numbers, which changes the
 * digest and fails a record that is perfectly sound.
 *
 * So a record is read twice — once by this scanner, to capture each event's
 * `metadata` span verbatim, and once by `JSON.parse`, for everything else.
 *
 * The scanner walks members at the top level of each object rather than
 * searching for `"metadata":`. A search would find a nested `metadata` key
 * first when one precedes the real member, and hash the wrong bytes.
 */

export class ParseError extends Error {
  constructor(message: string, readonly offset: number) {
    super(`${message} (at offset ${offset})`);
    this.name = "ParseError";
  }
}

const isWs = (c: string) => c === " " || c === "\t" || c === "\n" || c === "\r";

function skipWs(s: string, i: number): number {
  while (i < s.length && isWs(s[i]!)) i++;
  return i;
}

/** Index just past the closing quote of the string starting at `i`. */
function scanString(s: string, i: number): number {
  if (s[i] !== '"') throw new ParseError("expected a string", i);
  i++;
  while (i < s.length) {
    const c = s[i]!;
    if (c === "\\") {
      i += 2;
      continue;
    }
    if (c === '"') return i + 1;
    i++;
  }
  throw new ParseError("unterminated string", i);
}

/** Index just past the value starting at `i`. */
function scanValue(s: string, i: number): number {
  const c = s[i];
  if (c === undefined) throw new ParseError("expected a value", i);
  if (c === '"') return scanString(s, i);
  if (c === "{" || c === "[") {
    const close = c === "{" ? "}" : "]";
    let depth = 0;
    while (i < s.length) {
      const ch = s[i]!;
      if (ch === '"') {
        i = scanString(s, i);
        continue;
      }
      if (ch === "{" || ch === "[") depth++;
      else if (ch === "}" || ch === "]") {
        depth--;
        if (depth === 0) {
          if (ch !== close) throw new ParseError("mismatched bracket", i);
          return i + 1;
        }
      }
      i++;
    }
    throw new ParseError("unterminated object or array", i);
  }
  // number, true, false, null
  const start = i;
  while (i < s.length && !isWs(s[i]!) && s[i] !== "," && s[i] !== "}" && s[i] !== "]") i++;
  if (i === start) throw new ParseError("expected a value", i);
  return i;
}

/**
 * The raw text of a top-level member's value, or `undefined` when absent.
 *
 * Exported because an auditor verifying an export by hand needs exactly this.
 */
export function rawMember(objectText: string, key: string): string | undefined {
  let i = skipWs(objectText, 0);
  if (objectText[i] !== "{") throw new ParseError("expected an object", i);
  i = skipWs(objectText, i + 1);
  if (objectText[i] === "}") return undefined;

  while (i < objectText.length) {
    const keyStart = i;
    const keyEnd = scanString(objectText, i);
    const found = JSON.parse(objectText.slice(keyStart, keyEnd)) as string;

    i = skipWs(objectText, keyEnd);
    if (objectText[i] !== ":") throw new ParseError("expected ':'", i);
    i = skipWs(objectText, i + 1);

    const valueStart = i;
    const valueEnd = scanValue(objectText, i);
    if (found === key) return objectText.slice(valueStart, valueEnd);

    i = skipWs(objectText, valueEnd);
    if (objectText[i] === ",") {
      i = skipWs(objectText, i + 1);
      continue;
    }
    if (objectText[i] === "}") return undefined;
    throw new ParseError("expected ',' or '}'", i);
  }
  throw new ParseError("unterminated object", i);
}

/** The raw text of each element of a top-level JSON array. */
export function splitArray(arrayText: string): string[] {
  let i = skipWs(arrayText, 0);
  if (arrayText[i] !== "[") throw new ParseError("expected an array", i);
  i = skipWs(arrayText, i + 1);
  const out: string[] = [];
  if (arrayText[i] === "]") return out;

  while (i < arrayText.length) {
    const start = i;
    const end = scanValue(arrayText, i);
    out.push(arrayText.slice(start, end));
    i = skipWs(arrayText, end);
    if (arrayText[i] === ",") {
      i = skipWs(arrayText, i + 1);
      continue;
    }
    if (arrayText[i] === "]") return out;
    throw new ParseError("expected ',' or ']'", i);
  }
  throw new ParseError("unterminated array", i);
}
