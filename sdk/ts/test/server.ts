import { spawn, type ChildProcess } from "node:child_process";
import { fileURLToPath } from "node:url";
import { existsSync } from "node:fs";

const BIN = fileURLToPath(new URL("../../../target/debug/tideline-server", import.meta.url));

/**
 * Start a reference server on a free port and return its base URL.
 *
 * The SDK is tested against the real server, not a mock. A mock would agree
 * with whatever the SDK believes, which is exactly the belief under test.
 */
export async function startServer(): Promise<{ base: string; stop: () => void }> {
  if (!existsSync(BIN)) {
    throw new Error(`${BIN} is missing — run \`cargo build --bin tideline-server\` first`);
  }
  const port = 9000 + Math.floor(Math.random() * 900);
  const child: ChildProcess = spawn(BIN, [], {
    env: {
      ...process.env,
      TIDELINE_PORT: String(port),
      RUST_LOG: "warn",
      // The server refuses to start with an in-memory record store unless told
      // that is intended, because losing the record is catastrophic in
      // production. For a test it is exactly what we want.
      TIDELINE_EPHEMERAL_RECORDS: "1",
    },
    stdio: "ignore",
  });

  const base = `http://127.0.0.1:${port}`;
  for (let i = 0; i < 200; i++) {
    try {
      const res = await fetch(`${base}/health`);
      if (res.ok) return { base, stop: () => child.kill("SIGTERM") };
    } catch {
      /* not up yet */
    }
    await new Promise((r) => setTimeout(r, 25));
  }
  child.kill("SIGKILL");
  throw new Error(`server did not become healthy on ${base}`);
}
