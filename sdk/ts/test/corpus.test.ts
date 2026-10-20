import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { canon, hashEvent } from "../src/canon.js";
import { verifyChain, ChainError } from "../src/chain.js";
import { parseEvent, parseRecord } from "../src/event.js";
import { sha256, toHex } from "../src/sha256.js";

const load = (name: string) =>
  JSON.parse(
    readFileSync(fileURLToPath(new URL(`../../../conformance/corpus/${name}`, import.meta.url)), "utf8"),
  );

describe("event vectors", () => {
  const corpus = load("events.json");

  it("has not shrunk", () => {
    expect(corpus.vectors.length).toBeGreaterThanOrEqual(8);
  });

  for (const v of corpus.vectors) {
    it(v.name, () => {
      // parseEvent, not JSON.parse: the metadata bytes must survive.
      const event = parseEvent(v.event_json);
      expect(toHex(sha256(canon(event))), "canon").toBe(v.canon_sha256);
      expect(hashEvent(event, v.prev_hash), "hash").toBe(v.hash);
    });
  }
});

describe("chain cases", () => {
  const corpus = load("chains.json");

  for (const c of corpus.cases) {
    it(c.name, () => {
      // From the case's source text, never a re-serialisation of it.
      const events = parseRecord(c.events_json);
      if (c.expect === "ok") {
        expect(() => verifyChain(events)).not.toThrow();
        return;
      }
      try {
        verifyChain(events);
        throw new Error(`expected ${c.expect}`);
      } catch (e) {
        expect(e).toBeInstanceOf(ChainError);
        const err = e as ChainError;
        expect(err.reason).toBe(c.expect);
        if (c.at_seq !== undefined) expect(err.seq).toBe(c.at_seq);
      }
    });
  }
});
