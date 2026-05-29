import path from "node:path";
import { tmpdir } from "node:os";

export function binaryName(platform = process.platform): string {
  return platform === "win32" ? "procwatch.exe" : "procwatch";
}

export function targetPlatform(): string {
  return `${process.platform}-${process.arch}`;
}

export function targetTriple(
  platform = process.platform,
  arch = process.arch
): string {
  const normalizedArch = arch === "arm64" ? "aarch64" : "x86_64";
  if (platform === "darwin") return `${normalizedArch}-apple-darwin`;
  if (platform === "linux") return `${normalizedArch}-unknown-linux-gnu`;
  if (platform === "win32") return `${normalizedArch}-pc-windows-msvc`;
  throw new Error(`unsupported platform: ${platform}/${arch}`);
}

export function cacheRoot(env = process.env): string {
  if (env.PROCWATCH_CACHE_DIR) return env.PROCWATCH_CACHE_DIR;
  if (process.platform === "darwin") {
    return path.join(env.HOME || tmpdir(), "Library", "Caches", "procwatch", "bin");
  }
  if (process.platform === "win32") {
    return path.join(env.LOCALAPPDATA || tmpdir(), "procwatch", "Cache", "bin");
  }
  return path.join(
    env.XDG_CACHE_HOME || path.join(env.HOME || tmpdir(), ".cache"),
    "procwatch",
    "bin"
  );
}

export function archiveFileName(version: string, triple = targetTriple()): string {
  const ext = process.platform === "win32" ? "zip" : "tar.gz";
  return `procwatch-v${version}-${triple}.${ext}`;
}
