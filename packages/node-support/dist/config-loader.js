import { pathToFileURL } from "node:url";
import { createRequire } from "node:module";
import { fail, writeJson } from "./protocol.js";
async function loadConfig(configPath) {
    const ext = configPath.split(".").pop();
    if (ext === "cjs" || ext === "cts") {
        const require = createRequire(import.meta.url);
        const mod = require(configPath);
        return mod?.default ?? mod;
    }
    const mod = await import(pathToFileURL(configPath).href);
    return mod?.default ?? mod;
}
async function main() {
    const arg = process.argv[2];
    if (!arg || arg === "--help") {
        writeJson({ usage: "config-loader <ecosystem-config-path>" });
        return;
    }
    try {
        const loaded = await loadConfig(arg);
        writeJson(loaded);
    }
    catch (error) {
        fail(error instanceof Error ? error.message : String(error));
    }
}
await main();

