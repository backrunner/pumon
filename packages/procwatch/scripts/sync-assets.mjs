import { copyFile, mkdir, stat } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";

const here = path.dirname(fileURLToPath(import.meta.url));
const packageRoot = path.resolve(here, "..");
const repoRoot = path.resolve(packageRoot, "../..");

const assets = [
  {
    source: path.join(repoRoot, "packages", "node-support", "dist", "config-loader.js"),
    target: path.join(packageRoot, "vendor", "node-support", "config-loader.js")
  },
  {
    source: path.join(repoRoot, "packages", "node-support", "dist", "package-json.js"),
    target: path.join(packageRoot, "vendor", "node-support", "package-json.js")
  },
  {
    source: path.join(repoRoot, "packages", "node-support", "dist", "protocol.js"),
    target: path.join(packageRoot, "vendor", "node-support", "protocol.js")
  },
  {
    source: path.join(repoRoot, "packages", "cluster-shim", "dist", "index.js"),
    target: path.join(packageRoot, "vendor", "cluster-shim", "index.js")
  }
];

await Promise.all(
  assets.map(async ({ source, target }) => {
    await stat(source);
    await mkdir(path.dirname(target), { recursive: true });
    await copyFile(source, target);
  })
);

console.log("Synced Procwatch runtime assets into vendor/");
