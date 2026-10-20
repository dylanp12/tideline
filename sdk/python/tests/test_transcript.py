"""The shared HTTP transcript, driven from Python."""

import json
import os
import pathlib
import urllib.error
import urllib.request

from tideline import parse_record, verify_chain

ROOT = pathlib.Path(__file__).resolve().parents[3]
SCRIPT = json.loads(
    (ROOT / "conformance" / "transcript" / "basic.json").read_text(encoding="utf-8")
)


def subst_string(s, variables):
    for k, v in variables.items():
        s = s.replace(f"${k}", str(v))
    return s


def subst(value, variables):
    """A string that is exactly ``$VAR`` takes the variable's value, so a
    captured number stays a number rather than becoming a string."""
    if isinstance(value, str):
        if value.startswith("$") and value[1:] in variables:
            return variables[value[1:]]
        return subst_string(value, variables)
    if isinstance(value, list):
        return [subst(v, variables) for v in value]
    if isinstance(value, dict):
        return {k: subst(v, variables) for k, v in value.items()}
    return value


def test_transcript_passes(server):
    variables = {"RUN": f"py-conformance-{os.getpid()}"}

    for step in SCRIPT["steps"]:
        name = step["name"]
        req = step["request"]
        path = subst_string(req["path"], variables)

        status, headers, text = 0, {}, ""
        for _ in range(step.get("repeat", 1)):
            data, hdrs = None, dict(req.get("headers") or {})
            if "body" in req:
                data = json.dumps(subst(req["body"], variables)).encode()
                hdrs["content-type"] = "application/json"
            request = urllib.request.Request(
                f"{server}{path}", data=data, headers=hdrs, method=req["method"]
            )
            try:
                with urllib.request.urlopen(request) as res:
                    status, headers, text = res.status, dict(res.headers), res.read().decode()
            except urllib.error.HTTPError as e:
                status, headers, text = e.code, dict(e.headers), e.read().decode()

        expect = step.get("expect", {})
        if "status" in expect:
            assert status == expect["status"], f"{name}: body {text}"
        for key, want in (expect.get("header_eq") or {}).items():
            assert headers.get(key) == want, f"{name}: header {key}"

        needs_body = any(
            k in expect for k in ("json_has", "json_eq", "array_len", "chain_verifies")
        ) or "capture" in step
        if not needs_body:
            continue

        body = json.loads(text)
        for key in expect.get("json_has", []):
            assert key in body, f"{name}: missing {key}"
        for key, want in (expect.get("json_eq") or {}).items():
            assert body[key] == subst(want, variables), f"{name}: {key}"
        if "array_len" in expect:
            assert isinstance(body, list), f"{name}: expected an array"
            assert len(body) == expect["array_len"], f"{name}: array length"
        if expect.get("chain_verifies"):
            # From the response text, never from the parsed body.
            chain = verify_chain(parse_record(text))
            assert chain.sealed, f"{name}: expected a sealed record"
        for var, field in (step.get("capture") or {}).items():
            variables[var] = body[field]
