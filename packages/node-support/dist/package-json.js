import { readFile } from "node:fs/promises";
import path from "node:path";
export async function readPackageJson(cwd) {
    try {
        const raw = await readFile(path.join(cwd, "package.json"), "utf8");
        return JSON.parse(raw);
    }
    catch {
        return null;
    }
}

