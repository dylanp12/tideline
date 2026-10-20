import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { startServer } from "./server.js";
import { parseRecord } from "../src/event.js";
import { verifyChain } from "../src/chain.js";

const script = JSON.parse(
  readFileSync(
    fileURLToPath(new URL("../../../conformance/transcript/basic.json", import.meta.url)),
    "utf8",
  ),
);

let server: { base: string; stop: () => void };
beforeAll(async () => {
  server = await startServer();
}, 60_000);
afterAll(() => server?.stop());

/** Substitute `$VAR` inside a string. */
function substString(s: string, vars: Map<string, unknown>): string {
  let out = s;
  for (const [k, v] of vars) out = out.replaceAll(`$${k}`, String(v));
  return out;
}

/** Substitute recursively. A string that is exactly `$VAR` takes the variable's
 *  value, so a captured number stays a number. */
function subst(v: unknown, vars: Map<string, unknown>): unknown {
  if (typeof v === "string") {
    if (v.startsWith("$") && vars.has(v.slice(1))) return vars.get(v.slice(1));
    return substString(v, vars);
  }
  if (Array.isArray(v)) return v.map((x) => subst(x, vars));
  if (v && typeof v === "object") {
    return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, subst(x, vars)]));
  }
  return v;
}

describe("TLR/1 transcript", () => {
  it("passes against a live server", async () => {
    const vars = new Map<string, unknown>([["RUN", `ts-conformance-${process.pid}`]]);

    for (const step of script.steps) {
      const name: string = step.name;
      const req = step.request;
      const path = substString(req.path, vars);
      const repeat: number = step.repeat ?? 1;

      let status = 0;
      let headers: Headers = new Headers();
      let text = "";
      for (let i = 0; i < repeat; i++) {
        const init: RequestInit = { method: req.method, headers: { ...(req.headers ?? {}) } };
        if (req.body !== undefined) {
          init.body = JSON.stringify(subst(req.body, vars));
          (init.headers as Record<string, string>)["content-type"] = "application/json";
        }
        const res = await fetch(`${server.base}${path}`, init);
        status = res.status;
        headers = res.headers;
        text = await res.text();
      }

      const expect_ = step.expect ?? {};
      if (expect_.status !== undefined) {
        expect(status, `${name}: body ${text}`).toBe(expect_.status);
      }
      for (const [k, v] of Object.entries(expect_.header_eq ?? {})) {
        expect(headers.get(k), `${name}: header ${k}`).toBe(v);
      }

      const needsBody =
        expect_.json_has || expect_.json_eq || expect_.array_len !== undefined ||
        expect_.chain_verifies || step.capture;
      if (!needsBody) continue;

      const body = JSON.parse(text);
      for (const k of expect_.json_has ?? []) {
        expect(body[k], `${name}: missing ${k}`).toBeDefined();
      }
      for (const [k, v] of Object.entries(expect_.json_eq ?? {})) {
        expect(body[k], `${name}: ${k}`).toStrictEqual(subst(v, vars));
      }
      if (expect_.array_len !== undefined) {
        expect(Array.isArray(body), `${name}: expected an array`).toBe(true);
        expect(body.length, `${name}: array length`).toBe(expect_.array_len);
      }
      if (expect_.chain_verifies) {
        // From the response text, never from the parsed body.
        const chain = verifyChain(parseRecord(text));
        expect(chain.sealed, `${name}: expected a sealed record`).toBe(true);
      }
      for (const [v, field] of Object.entries(step.capture ?? {})) {
        vars.set(v, body[field as string]);
      }
    }
  }, 60_000);
});
