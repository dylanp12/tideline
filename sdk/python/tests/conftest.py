"""A real reference server for the client tests.

The SDK is tested against the server, not a mock. A mock would agree with
whatever the SDK believes, which is exactly the belief under test.
"""

import pathlib
import random
import subprocess
import time
import urllib.error
import urllib.request

import pytest

ROOT = pathlib.Path(__file__).resolve().parents[3]
BIN = ROOT / "target" / "debug" / "tideline-server"


@pytest.fixture(scope="session")
def server() -> str:
    if not BIN.exists():
        pytest.skip(f"{BIN} is missing — run `cargo build --bin tideline-server` first")

    port = random.randint(9000, 9899)
    proc = subprocess.Popen(
        [str(BIN)],
        env={
            "PATH": "/usr/bin:/bin",
            "TIDELINE_PORT": str(port),
            "RUST_LOG": "warn",
            # The server refuses an in-memory record store unless told that is
            # intended: losing the record is catastrophic in production, and
            # exactly what a test wants.
            "TIDELINE_EPHEMERAL_RECORDS": "1",
        },
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    base = f"http://127.0.0.1:{port}"
    try:
        for _ in range(200):
            try:
                with urllib.request.urlopen(f"{base}/health", timeout=1) as r:
                    if r.status == 200:
                        break
            except (urllib.error.URLError, OSError):
                time.sleep(0.025)
        else:
            raise RuntimeError(f"server did not become healthy on {base}")
        yield base
    finally:
        proc.terminate()
        proc.wait(timeout=10)
