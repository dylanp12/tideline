import { describe, it, expect } from "vitest";
import { rawMember, splitArray, ParseError } from "../src/parse.js";
import { parseEvent, parseRecord } from "../src/event.js";

describe("rawMember", () => {
  it("returns the value's exact text", () => {
    expect(rawMember('{"a":1,"metadata":{ "z" : 1 }}', "metadata")).toBe('{ "z" : 1 }');
  });

  it("picks the top-level member, not a nested one with the same name", () => {
    // A substring search finds the nested key first and hashes the wrong bytes.
    const src = '{"seq":1,"redacted":{"metadata":"aa"},"metadata":{"real":true}}';
    expect(rawMember(src, "metadata")).toBe('{"real":true}');
  });

  it("is not fooled by the key name appearing inside a string value", () => {
    const src = '{"content":"see \\"metadata\\": below","metadata":{"real":1}}';
    expect(rawMember(src, "metadata")).toBe('{"real":1}');
  });

  it("returns undefined when the member is absent", () => {
    expect(rawMember('{"a":1}', "metadata")).toBeUndefined();
    expect(rawMember("{}", "metadata")).toBeUndefined();
  });

  it("handles every value shape", () => {
    const src = '{"s":"x","n":-1.5e3,"t":true,"z":null,"a":[1,{"b":2}],"o":{"c":[]}}';
    expect(rawMember(src, "s")).toBe('"x"');
    expect(rawMember(src, "n")).toBe("-1.5e3");
    expect(rawMember(src, "t")).toBe("true");
    expect(rawMember(src, "z")).toBe("null");
    expect(rawMember(src, "a")).toBe('[1,{"b":2}]');
    expect(rawMember(src, "o")).toBe('{"c":[]}');
  });

  it("rejects malformed input rather than guessing", () => {
    expect(() => rawMember('{"a"', "a")).toThrow(ParseError);
    expect(() => rawMember("not an object", "a")).toThrow(ParseError);
  });
});

describe("splitArray", () => {
  it("splits objects containing brackets inside strings", () => {
    const text = '[{"c":"]}["},{"c":"b"}]';
    expect(splitArray(text)).toEqual(['{"c":"]}["}', '{"c":"b"}']);
  });

  it("handles an empty array", () => {
    expect(splitArray("[]")).toEqual([]);
    expect(splitArray("  [ ]  ")).toEqual([]);
  });
});

describe("parseEvent", () => {
  it("keeps metadata bytes and still exposes a parsed value", () => {
    const e = parseEvent('{"seq":0,"ts":1,"kind":"run_started","metadata":{ "a" : 1 }}');
    expect(e.metadataRaw).toBe('{ "a" : 1 }');
    expect(e.metadata).toEqual({ a: 1 });
  });

  it("treats a null metadata member as absent", () => {
    // Absent and null are the same claim: no metadata. An empty digest, not a
    // digest of the four characters "null".
    const e = parseEvent('{"seq":0,"ts":1,"kind":"run_started","metadata":null}');
    expect(e.metadataRaw).toBeUndefined();
  });

  it("rejects an unknown kind rather than defaulting", () => {
    expect(() => parseEvent('{"seq":0,"ts":1,"kind":"wire_transfer"}')).toThrow(ParseError);
  });
});

describe("parseRecord", () => {
  it("round-trips a record of several events", () => {
    const text =
      '[{"seq":0,"ts":1,"kind":"run_started","metadata":{"a":1}},' +
      '{"seq":1,"ts":2,"kind":"message","content":"hi"}]';
    const events = parseRecord(text);
    expect(events).toHaveLength(2);
    expect(events[0]!.metadataRaw).toBe('{"a":1}');
    expect(events[1]!.content).toBe("hi");
  });
});
