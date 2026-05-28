#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "../../..");
const binary = path.join(repoRoot, "target", "debug", process.platform === "win32" ? "promon.exe" : "promon");

const result = spawnSync(binary, process.argv.slice(2), {
  stdio: "inherit"
});

if (result.error) {
  console.error(`promon binary not found at ${binary}`);
  console.error("Run `cargo build -p promon-cli` first.");
  process.exit(1);
}

process.exit(result.status ?? 1);

