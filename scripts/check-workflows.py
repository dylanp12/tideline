#!/usr/bin/env python3
"""Parse every workflow file.

A workflow with invalid YAML does not fail loudly: GitHub records a run with
zero jobs and a red cross, named after the file rather than the workflow, which
is easy to mistake for a test failure. This catches it before pushing.

    python3 scripts/check-workflows.py
"""

import pathlib
import sys

try:
    import yaml
except ImportError:
    print("  skipped — pyyaml is not installed")
    sys.exit(0)

failed = False
for path in sorted(pathlib.Path(".github/workflows").glob("*.yml")):
    try:
        parsed = yaml.safe_load(path.read_text())
        jobs = list(parsed.get("jobs", {}))
        if not jobs:
            print(f"  FAIL    {path} declares no jobs")
            failed = True
        else:
            print(f"  ok      {path} — {', '.join(jobs)}")
    except yaml.YAMLError as e:
        mark = getattr(e, "problem_mark", None)
        where = f" at line {mark.line + 1}, column {mark.column + 1}" if mark else ""
        print(f"  FAIL    {path}{where}: {getattr(e, 'problem', e)}")
        failed = True

sys.exit(1 if failed else 0)
