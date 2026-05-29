#!/usr/bin/env node
import { createHash } from "node:crypto";
import { createWriteStream, existsSync, mkdirSync, readFileSync } from "node:fs";
import { chmod, mkdtemp, rename, rm } from "node:fs/promises";
import { get } from "node:https";
import { tmpdir } from "node:os";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const here = path.dirname(fileURLToPath(import.meta.url));
const packageRoot = path.resolve(here, "..");
const repoRoot = path.resolve(packageRoot, "../..");
const packageJson = JSON.parse(readFileSync(path.join(packageRoot, "package.json"), "utf8"));
const localBinary = path.join(repoRoot, "target", "debug", process.platform === "win32" ? "promon.exe" : "promon");
const loader = path.join(repoRoot, "packages", "node-support", "dist", "config-loader.js");
const clusterShim = path.join(repoRoot, "packages", "cluster-shim", "dist", "index.js");

const binary = existsSync(localBinary) ? localBinary : await ensureReleaseBinary();
const result = spawnSync(binary, process.argv.slice(2), {
  stdio: "inherit",
  env: {
    ...process.env,
    PROMON_NODE_SUPPORT_LOADER: process.env.PROMON_NODE_SUPPORT_LOADER || loader,
    PROMON_CLUSTER_SHIM: process.env.PROMON_CLUSTER_SHIM || clusterShim
  }
});

if (result.error) {
  console.error(result.error.message);
  process.exit(1);
}

process.exit(result.status ?? 1);

async function ensureReleaseBinary() {
  const target = targetTriple();
  const cacheDir = path.join(cacheRoot(), packageJson.version, target);
  const binary = path.join(cacheDir, process.platform === "win32" ? "promon.exe" : "promon");
  if (existsSync(binary)) return binary;

  mkdirSync(cacheDir, { recursive: true });
  const archiveName = `promon-v${packageJson.version}-${target}.${process.platform === "win32" ? "zip" : "tar.gz"}`;
  const repo = process.env.PROMON_GITHUB_REPOSITORY || "backrunner/promon";
  const base = `https://github.com/${repo}/releases/download/v${packageJson.version}`;
  const tmp = await mkdtemp(path.join(tmpdir(), "promon-download-"));
  const archive = path.join(tmp, archiveName);
  await download(`${base}/${archiveName}`, archive);
  await verifyChecksum(`${base}/promon-v${packageJson.version}-checksums.txt`, archiveName, archive);

  if (process.platform === "win32") {
    const unzip = spawnSync("powershell", ["-NoProfile", "-Command", `Expand-Archive -Force '${archive}' '${tmp}'`], { stdio: "inherit" });
    if (unzip.status !== 0) throw new Error("failed to extract Promon archive");
  } else {
    const tar = spawnSync("tar", ["-xzf", archive, "-C", tmp], { stdio: "inherit" });
    if (tar.status !== 0) throw new Error("failed to extract Promon archive");
  }

  const extracted = findExtractedBinary(tmp);
  await rename(extracted, binary);
  if (process.platform !== "win32") await chmod(binary, 0o755);
  await rm(tmp, { recursive: true, force: true });
  return binary;
}

function targetTriple() {
  const arch = process.arch === "arm64" ? "aarch64" : "x86_64";
  if (process.platform === "darwin") return `${arch}-apple-darwin`;
  if (process.platform === "linux") return `${arch}-unknown-linux-gnu`;
  if (process.platform === "win32") return `${arch}-pc-windows-msvc`;
  throw new Error(`unsupported platform: ${process.platform}/${process.arch}`);
}

function cacheRoot() {
  if (process.env.PROMON_CACHE_DIR) return process.env.PROMON_CACHE_DIR;
  if (process.platform === "darwin") return path.join(process.env.HOME || tmpdir(), "Library", "Caches", "promon", "bin");
  if (process.platform === "win32") return path.join(process.env.LOCALAPPDATA || tmpdir(), "promon", "Cache", "bin");
  return path.join(process.env.XDG_CACHE_HOME || path.join(process.env.HOME || tmpdir(), ".cache"), "promon", "bin");
}

function download(url, dest) {
  return new Promise((resolve, reject) => {
    get(url, (response) => {
      if (response.statusCode && response.statusCode >= 300 && response.statusCode < 400 && response.headers.location) {
        download(response.headers.location, dest).then(resolve, reject);
        return;
      }
      if (response.statusCode !== 200) {
        reject(new Error(`download failed ${response.statusCode}: ${url}`));
        return;
      }
      const file = createWriteStream(dest);
      response.pipe(file);
      file.on("finish", () => file.close(resolve));
      file.on("error", reject);
    }).on("error", reject);
  });
}

async function verifyChecksum(url, archiveName, archive) {
  const checksumFile = path.join(path.dirname(archive), "checksums.txt");
  await download(url, checksumFile);
  const expected = readFileSync(checksumFile, "utf8")
    .split(/\r?\n/)
    .map((line) => line.trim().split(/\s+/))
    .find((parts) => parts.includes(archiveName))?.[0];
  if (!expected) throw new Error(`checksum missing for ${archiveName}`);
  const actual = createHash("sha256").update(readFileSync(archive)).digest("hex");
  if (actual !== expected) throw new Error(`checksum mismatch for ${archiveName}`);
}

function findExtractedBinary(root) {
  const candidates = [
    path.join(root, process.platform === "win32" ? "promon.exe" : "promon"),
    path.join(root, "bin", process.platform === "win32" ? "promon.exe" : "promon")
  ];
  const found = candidates.find(existsSync);
  if (!found) throw new Error("Promon binary missing from release archive");
  return found;
}
