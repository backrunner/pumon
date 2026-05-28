import { readFile } from "node:fs/promises";
import path from "node:path";

export interface PackageJson {
  packageManager?: string;
  scripts?: Record<string, string>;
}

export async function readPackageJson(cwd: string): Promise<PackageJson | null> {
  try {
    const raw = await readFile(path.join(cwd, "package.json"), "utf8");
    return JSON.parse(raw) as PackageJson;
  } catch {
    return null;
  }
}

