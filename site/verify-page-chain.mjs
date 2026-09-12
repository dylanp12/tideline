// Checks that the chain on site/index.html is real.
//
// The landing page computes canon and SHA-256 in the browser rather than
// showing a picture of a chain, so this re-implements exactly what that page
// does and checks it against the committed corpus. A page that shows a green
// tick it did not earn is worse than no page.
//
//   node site/verify-page-chain.mjs        (from the repository root)
import { readFileSync } from "node:fs";
import { webcrypto } from "node:crypto";
const crypto = webcrypto;

const enc = new TextEncoder();
const ZERO = new Uint8Array(32);
const sha = async b => new Uint8Array(await crypto.subtle.digest("SHA-256", b));
const shaText = s => sha(enc.encode(s));
const hex = b => [...b].map(x => x.toString(16).padStart(2, "0")).join("");

function u64be(n) {
  const out = new Uint8Array(8), view = new DataView(out.buffer);
  view.setUint32(0, Math.floor(n / 0x100000000));
  view.setUint32(4, n % 0x100000000);
  return out;
}

async function canon(e, seq) {
  const parts = [
    enc.encode("tlr1\n"), u64be(seq), u64be(e.ts),
    await shaText(e.kind),
    e.role     ? await shaText(e.role)     : ZERO,
    e.name     ? await shaText(e.name)     : ZERO,
    e.text     ? await shaText(e.text)     : ZERO,
    e.metadata ? await shaText(e.metadata) : ZERO,
  ];
  const out = new Uint8Array(181);
  let at = 0;
  for (const p of parts) { out.set(p, at); at += p.length; }
  return out;
}

async function hashEvent(e, seq, prev) {
  const c = await canon(e, seq);
  const joined = new Uint8Array(213);
  joined.set(c, 0); joined.set(prev, 181);
  return sha(joined);
}

// Check against the corpus vector the Rust, Python and Go suites all use.
const corpus = JSON.parse(readFileSync("conformance/corpus/events.json", "utf8"));
const v = corpus.vectors.find(x => x.name === "all-fields");
const e = JSON.parse(v.event_json);

const c = await canon(
  { ts: e.ts, kind: e.kind, role: e.role, name: e.name, text: e.content,
    metadata: '{"bureau":"experian","ms":410}' },
  e.seq,
);
const gotCanon = hex(await sha(c));
const prev = Uint8Array.from(v.prev_hash.match(/../g).map(h => parseInt(h, 16)));
const gotHash = hex(await hashEvent(
  { ts: e.ts, kind: e.kind, role: e.role, name: e.name, text: e.content,
    metadata: '{"bureau":"experian","ms":410}' },
  e.seq, prev,
));

console.log("canon length:", c.length);
console.log("canon  page:", gotCanon);
console.log("canon corpus:", v.canon_sha256, gotCanon === v.canon_sha256 ? "✓" : "✗ MISMATCH");
console.log("hash   page:", gotHash);
console.log("hash corpus:", v.hash, gotHash === v.hash ? "✓" : "✗ MISMATCH");
process.exit(gotCanon === v.canon_sha256 && gotHash === v.hash ? 0 : 1);
