#!/usr/bin/env python3
"""Check that every crate carries the metadata crates.io requires.

`cargo publish --dry-run` would catch this, but it resolves path dependencies
against the registry, so it only works for crates whose dependencies are already
published. Until the first release that is `tideline-proto` alone. This checks
the other three directly.

    python3 scripts/check-manifests.py
"""

import json
import subprocess
import sys

REQUIRED = ("description", "license", "repository", "readme")
CRATES = ("tideline-proto", "tideline-server", "tideline-sdk", "tideline-cli")


def main() -> int:
    meta = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--no-deps", "--format-version", "1"],
            capture_output=True, text=True, check=True,
        ).stdout
    )
    packages = {p["name"]: p for p in meta["packages"]}

    failed = False
    for name in CRATES:
        package = packages.get(name)
        if package is None:
            print(f"  MISSING {name} is not in the workspace")
            failed = True
            continue
        missing = [f for f in REQUIRED if not package.get(f)]
        if missing:
            print(f"  FAIL    {name} is missing: {', '.join(missing)}")
            failed = True
        else:
            print(f"  ok      {name}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
