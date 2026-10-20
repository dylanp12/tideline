import { defineConfig } from "tsup";

export default defineConfig({
  entry: ["src/index.ts", "src/react.ts"],
  format: ["esm", "cjs"],
  dts: true,
  clean: true,
  treeshake: true,
  // No runtime dependencies: the verifier has to run in a browser, in Node, and
  // in an edge runtime without anyone installing anything.
  external: ["react"],
});
