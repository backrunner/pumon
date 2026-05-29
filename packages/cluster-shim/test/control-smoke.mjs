import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const tempDir = mkdtempSync(path.join(os.tmpdir(), "procwatch-cluster-shim."));
const controlPath = path.join(tempDir, "cluster.addr");
const shim = path.join(root, "dist", "index.js");
const worker = path.join(root, "test", "worker.mjs");
const spec = {
  instances: 2,
  controlPath,
  worker: {
    script: worker,
    args: [],
    nodeArgs: [],
    interpreter: "node",
    interpreterArgs: [],
    cwd: root,
    env: {}
  }
};

const child = spawn(process.execPath, [shim, JSON.stringify(spec)], {
  stdio: ["ignore", "pipe", "pipe"]
});

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function waitForExit(processHandle, timeoutMs) {
  return new Promise((resolve) => {
    if (processHandle.exitCode !== null || processHandle.signalCode !== null) {
      resolve();
      return;
    }
    const timer = setTimeout(resolve, timeoutMs);
    processHandle.once("exit", () => {
      clearTimeout(timer);
      resolve();
    });
  });
}

async function waitForControlFile() {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    try {
      return JSON.parse(readFileSync(controlPath, "utf8"));
    } catch {
      await delay(100);
    }
  }
  throw new Error("cluster control file was not created");
}

function sendControl(address, request) {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection({ host: address.host, port: address.port });
    let buffer = "";
    const timer = setTimeout(() => {
      socket.destroy();
      reject(new Error(`control request timed out: ${request.type}`));
    }, 5000);
    socket.setEncoding("utf8");
    socket.on("connect", () => {
      socket.write(`${JSON.stringify({ ...request, token: address.token })}\n`);
    });
    socket.on("data", (chunk) => {
      buffer += chunk;
      const index = buffer.indexOf("\n");
      if (index === -1) return;
      clearTimeout(timer);
      socket.end();
      const response = JSON.parse(buffer.slice(0, index));
      if (!response.ok) {
        reject(new Error(response.error || `control request failed: ${request.type}`));
        return;
      }
      resolve(response);
    });
    socket.on("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
  });
}

try {
  const address = await waitForControlFile();
  if (address.pid !== child.pid) {
    throw new Error(`control pid mismatch: expected ${child.pid}, got ${address.pid}`);
  }

  const scale = await sendControl(address, { type: "scale", instances: 1 });
  if (scale.target !== 1) {
    throw new Error(`scale target mismatch: ${JSON.stringify(scale)}`);
  }

  const reload = await sendControl(address, { type: "reload" });
  if (reload.target !== 1) {
    throw new Error(`reload target mismatch: ${JSON.stringify(reload)}`);
  }
} finally {
  child.kill("SIGTERM");
  await waitForExit(child, 1000);
  if (child.exitCode === null) child.kill("SIGKILL");
  await waitForExit(child, 1000);
  rmSync(tempDir, { recursive: true, force: true });
}
