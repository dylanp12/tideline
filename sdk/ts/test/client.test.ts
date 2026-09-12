import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { startServer } from "./server.js";
import { Tideline, TidelineError } from "../src/client.js";
import { ChainError, verifyChain } from "../src/chain.js";

let server: { base: string; stop: () => void };
let tl: Tideline;

beforeAll(async () => {
  server = await startServer();
  tl = new Tideline({ url: server.base });
}, 60_000);
afterAll(() => server?.stop());

let n = 0;
const freshId = () => `client-${process.pid}-${n++}`;

async function newRun() {
  return tl.startRun({
    runId: freshId(),
    agent: { name: "underwriter", version: "2.1.0" },
    subjectRef: "applicant-4821",
    labels: { product: "personal-loan" },
  });
}

describe("Tideline client", () => {
  it("records a run and verifies it", async () => {
    const run = await newRun();
    await run.record({
      kind: "tool_call",
      role: "tool",
      name: "pull_credit_report",
      content: "score=690",
      metadata: { bureau: "experian", ms: 410 },
    });
    await run.complete();

    const { events, chain } = await run.verifiedEvents();
    expect(chain.sealed).toBe(true);
    expect(chain.len).toBe(3);
    expect(events[1]!.kind).toBe("tool_call");
    // The metadata survived the round trip byte for byte, which is the only
    // reason the record verifies.
    expect(events[1]!.metadataRaw).toBe('{"bureau":"experian","ms":410}');
  });

  it("reports the head in the envelope", async () => {
    const run = await newRun();
    await run.record({ kind: "message", content: "x" });
    const env = await run.envelope();
    expect(env.head_seq).toBe(1);
    expect(env.agent.name).toBe("underwriter");
  });

  it("appends once for a repeated idempotency key", async () => {
    const run = await newRun();
    const a = await run.record({ kind: "message", content: "once" }, { idempotencyKey: "k-1" });
    const b = await run.record({ kind: "message", content: "once" }, { idempotencyKey: "k-1" });
    expect(a.seq).toBe(b.seq);
    expect(a.hash).toBe(b.hash);
    expect((await run.events()).length).toBe(2);
  });

  it("blocks on a gate until a reviewer decides", async () => {
    const run = await newRun();

    // A reviewer turns up a moment later, as one does.
    const reviewer = tl.run(run.id);
    void (async () => {
      for (let i = 0; i < 100; i++) {
        const pending = await reviewer.approvals().catch(() => []);
        const g = pending[0];
        if (g) {
          await reviewer.resolve(g.seq, {
            decision: "approved",
            reviewer: "Jane Okafor",
            note: "within policy",
          });
          return;
        }
        await new Promise((r) => setTimeout(r, 20));
      }
    })();

    const decision = await run.gate({ action: "Approve EUR40,000 loan", expiresIn: 60 });
    expect(decision.approved).toBe(true);
    expect(decision.reviewer).toBe("Jane Okafor");
    // No control plane configured, so the server cannot vouch for who that was.
    expect(decision.attested).toBe(false);

    expect(() => verifyChain([])).toThrow();
    verifyChain(await run.events());
  }, 30_000);

  it("resolves a rejection rather than throwing", async () => {
    // An agent must be able to branch on a refusal.
    const run = await newRun();
    const { seq } = await run.openGate({ action: "Approve EUR900,000", expiresIn: 60 });
    await run.resolve(seq, { decision: "rejected", reviewer: "Risk", note: "exceeds mandate" });

    const decision = await run.awaitGate(seq);
    expect(decision.decision).toBe("rejected");
    expect(decision.approved).toBe(false);
  });

  it("surfaces a second resolution as a conflict", async () => {
    const run = await newRun();
    const { seq } = await run.openGate({ action: "act", expiresIn: 60 });
    await run.resolve(seq, { decision: "approved" });

    await expect(run.resolve(seq, { decision: "rejected" })).rejects.toSatisfy(
      (e: unknown) => e instanceof TidelineError && e.isConflict,
    );
  });

  it("names the altered event when a record is tampered with", async () => {
    const run = await newRun();
    for (let i = 0; i < 4; i++) await run.record({ kind: "message", content: `m${i}` });

    const events = await run.events();
    events[3]!.content = "altered after the fact";

    try {
      verifyChain(events);
      throw new Error("expected verification to fail");
    } catch (e) {
      expect(e).toBeInstanceOf(ChainError);
      expect((e as ChainError).reason).toBe("hash_mismatch");
      expect((e as ChainError).seq).toBe(3);
    }
  });

  it("keeps the record verifiable through a redaction", async () => {
    const run = await newRun();
    await run.record({ kind: "tool_call", name: "kyc", content: "applicant dossier" });
    await run.redact(1, ["content"], "GDPR Art 17 request #55");

    const { events } = await run.verifiedEvents();
    expect(events[1]!.content).toBeUndefined();
    expect(events[1]!.redacted?.["content"]).toBeDefined();
  });

  it("checkpoints on seal, under the published key id", async () => {
    const run = await newRun();
    expect(await run.checkpoint()).toBeNull();
    await run.complete();

    const cp = await run.checkpoint();
    expect(cp).not.toBeNull();
    const doc = await tl.wellKnown();
    expect(cp!.key_id).toBe(doc.keys[0]!.id);
    expect(doc.protocol_versions).toContain(1);
  });

  it("streams the record over watch", async () => {
    const run = await newRun();
    await run.record({ kind: "message", content: "first" });

    const seen = [];
    for await (const e of run.watch()) {
      seen.push(e);
      if (seen.length === 2) break;
    }
    expect(seen[0]!.kind).toBe("run_started");
    expect(seen[1]!.content).toBe("first");
  }, 30_000);

  it("lists and filters runs", async () => {
    const run = await newRun();
    const page = await tl.listRuns({ agent: "underwriter" });
    expect(page.runs.some((r) => r.run_id === run.id)).toBe(true);

    const none = await tl.listRuns({ agent: "no-such-agent" });
    expect(none.runs.length).toBe(0);
  });

  it("reports an unknown run as not found", async () => {
    await expect(tl.run("nope-nope").envelope()).rejects.toSatisfy(
      (e: unknown) => e instanceof TidelineError && e.isNotFound,
    );
  });
});
