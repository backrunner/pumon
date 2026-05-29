import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { createWriteStream, existsSync, mkdirSync, readFileSync } from "node:fs";
import { chmod, mkdtemp, rename, rm } from "node:fs/promises";
import { get } from "node:https";
import path from "node:path";
import { tmpdir } from "node:os";

import { archiveFileName, binaryName, cacheRoot, targetTriple } from "./platform.js";

export interface DownloadBinaryOptions {
  version: string;
  repository?: string;
  cacheDir?: string;
}

export async function downloadBinary(options: DownloadBinaryOptions): Promise<string> {
  const version = options.version;
  const repository = options.repository || process.env.PROCWATCH_GITHUB_REPOSITORY || "backrunner/pumon";
  const triple = targetTriple();
  const root = options.cacheDir || cacheRoot();
  const binaryPath = path.join(root, version, triple, binaryName());
  if (existsSync(binaryPath)) return binaryPath;

  mkdirSync(path.dirname(binaryPath), { recursive: true });
  const archive = archiveFileName(version, triple);
  const base = `https://github.com/${repository}/releases/download/v${version}`;
  const tempDir = await mkdtemp(path.join(tmpdir(), "procwatch-download-"));
  const archivePath = path.join(tempDir, archive);

  try {
    await download(`${base}/${archive}`, archivePath);
    await verifyChecksum(
      `${base}/procwatch-v${version}-checksums.txt`,
      archive,
      archivePath
    );
    extractArchive(archivePath, tempDir);

    const extracted = findExtractedBinary(tempDir);
    await rename(extracted, binaryPath);
    if (process.platform !== "win32") {
      await chmod(binaryPath, 0o755);
    }
    return binaryPath;
  } finally {
    await rm(tempDir, { recursive: true, force: true });
  }
}

function extractArchive(archivePath: string, outputDir: string): void {
  if (process.platform === "win32") {
    const result = spawnSync(
      "powershell",
      ["-NoProfile", "-Command", `Expand-Archive -Force '${archivePath}' '${outputDir}'`],
      { stdio: "inherit" }
    );
    if (result.status !== 0) {
      throw new Error("failed to extract Procwatch archive");
    }
    return;
  }

  const result = spawnSync("tar", ["-xzf", archivePath, "-C", outputDir], {
    stdio: "inherit"
  });
  if (result.status !== 0) {
    throw new Error("failed to extract Procwatch archive");
  }
}

function download(url: string, dest: string): Promise<void> {
  return new Promise((resolve, reject) => {
    get(url, (response) => {
      if (
        response.statusCode &&
        response.statusCode >= 300 &&
        response.statusCode < 400 &&
        response.headers.location
      ) {
        download(response.headers.location, dest).then(resolve, reject);
        return;
      }
      if (response.statusCode !== 200) {
        reject(new Error(`download failed ${response.statusCode}: ${url}`));
        return;
      }
      const file = createWriteStream(dest);
      response.pipe(file);
      file.on("finish", () => file.close(() => resolve()));
      file.on("error", reject);
    }).on("error", reject);
  });
}

async function verifyChecksum(
  checksumUrl: string,
  archiveName: string,
  archivePath: string
): Promise<void> {
  const checksumPath = path.join(path.dirname(archivePath), "checksums.txt");
  await download(checksumUrl, checksumPath);
  const expected = readFileSync(checksumPath, "utf8")
    .split(/\r?\n/)
    .map((line) => line.trim().split(/\s+/))
    .find((parts) => parts.includes(archiveName))?.[0];
  if (!expected) {
    throw new Error(`checksum missing for ${archiveName}`);
  }

  const actual = createHash("sha256")
    .update(readFileSync(archivePath))
    .digest("hex");
  if (actual !== expected) {
    throw new Error(`checksum mismatch for ${archiveName}`);
  }
}

function findExtractedBinary(root: string): string {
  const file = binaryName();
  const candidates = [path.join(root, file), path.join(root, "bin", file)];
  const found = candidates.find(existsSync);
  if (!found) {
    throw new Error("Procwatch binary missing from release archive");
  }
  return found;
}
