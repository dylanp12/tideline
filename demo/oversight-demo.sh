#!/usr/bin/env bash
#
# Tideline oversight demo — a lending agent whose high-risk decision is gated by
# a human, producing a complete EU AI Act Article 12 (record) + Article 14
# (human oversight) audit trail that anyone can verify without trusting the
# server that produced it. One command, end to end.
#
#   ./demo/oversight-demo.sh
#
# Uses a running engine at $TIDELINE_BASE (default localhost:8080), or builds
# and starts one for the duration of the demo.

set -euo pipefail
cd "$(dirname "$0")/.."

BASE="${TIDELINE_BASE:-localhost:8080}"
RUN="loan-demo-$$"
STARTED=""

pp() { python3 -m json.tool 2>/dev/null || cat; }
post() { curl -s -XPOST "$BASE/$1" -H 'content-type: application/json' --data "$2"; }
say() { printf '\n\033[1m%s\033[0m\n' "$1"; }

# Bring up an engine if one isn't already listening.
if ! curl -sf "$BASE/health" >/dev/null 2>&1; then
  echo "starting engine (cargo build)…"
  cargo build -q --bin tideline-server
  ./target/debug/tideline-server >/tmp/tideline-oversight-demo.log 2>&1 &
  STARTED=$!
  curl --retry 60 --retry-connrefused --retry-delay 0 -sf "$BASE/health" >/dev/null
fi
trap '[ -n "$STARTED" ] && kill "$STARTED" 2>/dev/null || true' EXIT

echo "engine: $BASE   run: $RUN"

say "1. Open the run — the envelope is the first link in the chain"
post "v1/runs" "{\"run_id\":\"$RUN\",\"agent\":{\"name\":\"underwriter-agent\",\"version\":\"2.1.0\"},\"subject_ref\":\"applicant-4821\",\"labels\":{\"product\":\"personal-loan\",\"jurisdiction\":\"IE\"}}" | pp

say "2. The agent works the application (each step chained as it goes)"
post "v1/runs/$RUN/events" '{"kind":"message","role":"user","content":"Applicant #4821 requests a EUR40,000 personal loan; credit score 690, DTI 38%"}' >/dev/null
post "v1/runs/$RUN/events" '{"kind":"tool_call","role":"tool","name":"pull_credit_report","content":"score=690 utilisation=42% delinquencies=0","metadata":{"bureau":"experian"}}' >/dev/null
post "v1/runs/$RUN/events" '{"kind":"model_call","role":"assistant","name":"underwriter-agent","content":"Borderline: within policy but above the EUR25k auto-approve ceiling. Recommend approval with human sign-off.","metadata":{"model":"claude-opus-4-8","cost_usd":0.014}}' >/dev/null
echo "  recorded 3 events (message, tool_call, model_call)"

say "3. The agent reaches a high-risk action and OPENS A GATE (Art 14)"
SEQ=$(post "v1/runs/$RUN/approvals" '{"action":"Approve EUR40,000 loan for applicant #4821","expires_in":7200}' \
      | python3 -c 'import sys,json;print(json.load(sys.stdin)["seq"])')
echo "  gate opened at seq $SEQ — the agent is now blocked"

say "4. Reviewer queue"
curl -s "$BASE/v1/runs/$RUN/approvals" | pp

say "5. A credit-risk officer reviews and APPROVES"
post "v1/runs/$RUN/approvals/$SEQ/resolve" '{"decision":"approved","reviewer":"Jane Okafor (Credit Risk)","note":"Income verified; DTI within policy; manual sign-off on file"}' | pp

say "6. The agent proceeds, then seals the run"
post "v1/runs/$RUN/events" '{"kind":"decision","name":"loan_approved","content":"EUR40,000 approved for applicant #4821"}' >/dev/null
post "v1/runs/$RUN/complete" '' | pp

say "7. The complete Article 12 + 14 record"
curl -s "$BASE/v1/runs/$RUN/events" | pp

# The beat that distinguishes this from a log: anyone can check the record
# was not altered, without trusting the server that served it.
say "8. Verify the chain — no server, no trust"
EVIDENCE="${TMPDIR:-/tmp}/$RUN-evidence.json"
curl -s "$BASE/v1/runs/$RUN/events" >"$EVIDENCE"
python3 - "$EVIDENCE" <<'PYV'
import hashlib, json, struct, sys

ZERO = b"\x00" * 32
h = lambda b: hashlib.sha256(b).digest()

def spans(src, opener="{"):
    """Yield the raw substring of each top-level object in a JSON array.

    Canon commits to the bytes the server sent, so the metadata must be read
    out of the response text. Parsing and re-serialising would normalise
    spacing and number formatting and produce a different hash.
    """
    closer = "}" if opener == "{" else "]"
    depth = start = 0
    instr = esc = False
    for i, c in enumerate(src):
        if instr:
            if esc: esc = False
            elif c == "\\": esc = True
            elif c == '"': instr = False
            continue
        if c == '"': instr = True
        elif c == opener:
            if depth == 0: start = i
            depth += 1
        elif c == closer:
            depth -= 1
            if depth == 0: yield src[start:i+1]

def member(src, key):
    """The raw byte span of one member's value, or None."""
    i = src.find(f'"{key}":')
    if i < 0: return None
    j = i + len(key) + 3
    while src[j] == " ": j += 1
    if src[j] not in "{[": return None
    return next(spans(src[j:], src[j]))

raw = open(sys.argv[1], encoding="utf-8").read()
prev, n = ZERO, 0
for i, src in enumerate(spans(raw)):
    e = json.loads(src)
    red = e.get("redacted") or {}
    def digest(field):
        v = e.get(field)
        if v is not None: return h(v.encode("utf-8"))
        if field in red:  return bytes.fromhex(red[field])
        return ZERO
    meta = member(src, "metadata")
    md = h(meta.encode("utf-8")) if meta is not None else (
         bytes.fromhex(red["metadata"]) if "metadata" in red else ZERO)
    canon = (b"tlr1\n" + struct.pack(">Q", e["seq"]) + struct.pack(">Q", e["ts"])
             + h(e["kind"].encode()) + digest("role") + digest("name")
             + digest("content") + md)
    assert len(canon) == 181
    assert e["seq"] == i, f"seq gap at {i}"
    assert bytes.fromhex(e["prev_hash"]) == prev, f"broken link at seq {i}"
    got = hashlib.sha256(canon + prev).digest()
    assert got == bytes.fromhex(e["hash"]), f"altered event at seq {i}"
    prev, n = got, n + 1

print(f"  \u2713 {n} events verified \u2014 unbroken chain, head {prev.hex()[:16]}\u2026")
PYV

say "9. The server's signed checkpoint (closes the truncation gap)"
curl -s "$BASE/v1/runs/$RUN/checkpoint" | pp

say "Evidence exported → $EVIDENCE"
