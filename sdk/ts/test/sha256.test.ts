import { describe, it, expect } from "vitest";
import { createHash, randomBytes } from "node:crypto";
import { sha256, toHex, fromHex, sha256Utf8 } from "../src/sha256.js";

const native = (b: Uint8Array) => createHash("sha256").update(b).digest("hex");

describe("sha256", () => {
  it("matches the published vectors", () => {
    expect(toHex(sha256Utf8(""))).toBe(
      "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    );
    expect(toHex(sha256Utf8("abc"))).toBe(
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    );
  });

  it("agrees with node:crypto across the padding boundaries", () => {
    // 55/56 and 63/64 are where a wrong padding calculation hides: the message
    // plus 0x80 plus eight length bytes tips into a second block.
    for (const len of [0, 1, 31, 32, 54, 55, 56, 57, 63, 64, 65, 119, 120, 127, 128, 1000]) {
      const input = new Uint8Array(len).fill(0x61);
      expect(toHex(sha256(input)), `length ${len}`).toBe(native(input));
    }
  });

  it("agrees with node:crypto on random inputs", () => {
    for (let i = 0; i < 200; i++) {
      const input = new Uint8Array(randomBytes(Math.floor(Math.random() * 300)));
      expect(toHex(sha256(input))).toBe(native(input));
    }
  });

  it("hashes UTF-8, not code units", () => {
    const s = "€40 000 — ≈ 38% DTI 🏦";
    expect(toHex(sha256Utf8(s))).toBe(createHash("sha256").update(s, "utf8").digest("hex"));
  });

  it("round-trips hex", () => {
    const h = sha256Utf8("tideline");
    expect(fromHex(toHex(h))).toEqual(h);
  });

  it("rejects malformed hex", () => {
    expect(() => fromHex("abc")).toThrow();
    expect(() => fromHex("Z".repeat(64))).toThrow();
    expect(() => fromHex("A".repeat(64))).toThrow(); // uppercase is not the wire form
  });
});
