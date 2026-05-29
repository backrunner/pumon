import cluster from "node:cluster";
import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, renameSync, rmSync, writeFileSync } from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";

function help() {
  console.log("usage: procwatch-cluster-shim '<json-spec>'");
}

const raw = process.argv[2];
if (!raw || raw === "--help") {
  help();
  process.exit(0);
}

const spec = JSON.parse(raw);
const worker = spec.worker;
let targetInstances = spec.instances === "max" ? os.availableParallelism() : Math.max(1, Number(spec.instances || 1));
let shuttingDown = false;
let controlServer;
const controlToken = spec.controlToken || randomBytes(32).toString("hex");

function workerEnv(index) {
  return { ...(worker.env || {}), ...process.env, PROCWATCH_WORKER_ID: String(index) };
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function cleanupControlFile() {
  if (spec.controlPath && existsSync(spec.controlPath)) {
    rmSync(spec.controlPath, { force: true });
  }
}

function startControlServer(handler) {
  if (!spec.controlPath) return;
  mkdirSync(path.dirname(spec.controlPath), { recursive: true });
  cleanupControlFile();

  controlServer = net.createServer((socket) => {
    let buffer = "";
    socket.setEncoding("utf8");
    socket.on("data", (chunk) => {
      buffer += chunk;
      const lines = buffer.split(/\r?\n/);
      buffer = lines.pop() || "";
      for (const line of lines) {
        if (!line.trim()) continue;
        handleControlLine(handler, line, socket);
      }
    });
  });

  controlServer.listen(0, "127.0.0.1", () => {
    const address = controlServer.address();
    const tempPath = `${spec.controlPath}.${process.pid}.tmp`;
    writeFileSync(tempPath, JSON.stringify({ host: address.address, port: address.port, pid: process.pid, token: controlToken }));
    renameSync(tempPath, spec.controlPath);
  });

  process.on("exit", cleanupControlFile);
}

async function handleControlLine(handler, line, socket) {
  try {
    const request = JSON.parse(line);
    if (request.token !== controlToken) {
      throw new Error("invalid cluster control token");
    }
    const payload = await handler(request);
    socket.write(`${JSON.stringify({ ok: true, ...payload })}\n`);
  } catch (error) {
    socket.write(`${JSON.stringify({ ok: false, error: error instanceof Error ? error.message : String(error) })}\n`);
  }
}

function stopControlServer() {
  cleanupControlFile();
  if (controlServer) controlServer.close();
}

function installShutdown(handler) {
  const shutdown = () => {
    if (shuttingDown) return;
    shuttingDown = true;
    stopControlServer();
    handler();
    setTimeout(() => process.exit(0), 700).unref();
  };
  process.on("SIGTERM", shutdown);
  process.on("SIGINT", shutdown);
}

if ((worker.interpreter || "node") === "node") {
  if (cluster.isPrimary) {
    const workerIndexes = new Map();
    let nextIndex = 0;

    cluster.setupPrimary({
      exec: worker.script,
      args: worker.args || [],
      execArgv: [...(worker.nodeArgs || []), ...(worker.interpreterArgs || [])],
      cwd: worker.cwd
    });

    function activeWorkers() {
      return Object.values(cluster.workers || {}).filter(Boolean);
    }

    function forkWorker(index = nextIndex++) {
      const item = cluster.fork(workerEnv(index));
      workerIndexes.set(item.id, index);
      return item;
    }

    function stopWorker(item) {
      if (!item || item.isDead()) return;
      item.disconnect();
      setTimeout(() => {
        if (!item.isDead()) item.kill("SIGTERM");
      }, 5000).unref();
    }

    function waitWorkerReady(item) {
      return new Promise((resolve) => {
        const done = () => resolve();
        item.once("listening", done);
        item.once("online", done);
        setTimeout(done, 1000).unref();
      });
    }

    function waitWorkerExit(item) {
      return new Promise((resolve) => {
        if (!item || item.isDead()) {
          resolve();
          return;
        }
        item.once("exit", resolve);
        setTimeout(resolve, 6000).unref();
      });
    }

    async function scaleTo(count) {
      targetInstances = Math.max(1, Number(count || 1));
      const pending = [];
      while (activeWorkers().length < targetInstances) {
        pending.push(waitWorkerReady(forkWorker()));
      }
      if (pending.length > 0) {
        await Promise.all(pending);
      }
      const current = activeWorkers();
      if (current.length > targetInstances) {
        const excess = current.slice(targetInstances);
        for (const item of excess) stopWorker(item);
        await Promise.all(excess.map(waitWorkerExit));
      }
      return { workers: activeWorkers().length, target: targetInstances };
    }

    async function reloadWorkers() {
      const current = activeWorkers();
      for (const item of current) {
        const index = workerIndexes.get(item.id) ?? nextIndex++;
        const replacement = forkWorker(index);
        await waitWorkerReady(replacement);
        stopWorker(item);
        await waitWorkerExit(item);
      }
      return { workers: activeWorkers().length, target: targetInstances };
    }

    for (let i = 0; i < targetInstances; i += 1) forkWorker(i);
    nextIndex = targetInstances;

    cluster.on("exit", (item, code) => {
      const index = workerIndexes.get(item.id) ?? nextIndex++;
      workerIndexes.delete(item.id);
      if (!shuttingDown && !item.exitedAfterDisconnect && code !== 0 && activeWorkers().length < targetInstances) {
        forkWorker(index);
      }
    });

    startControlServer(async (request) => {
      if (request.type === "scale") return scaleTo(request.instances);
      if (request.type === "reload") return reloadWorkers();
      throw new Error(`unsupported cluster control request: ${request.type}`);
    });

    installShutdown(() => {
      for (const item of activeWorkers()) stopWorker(item);
    });
  }
} else {
  const children = new Set();
  let nextIndex = 0;

  function start(index = nextIndex++) {
    const child = spawn(worker.interpreter, [...(worker.interpreterArgs || []), worker.script, ...(worker.args || [])], {
      cwd: worker.cwd,
      env: workerEnv(index),
      stdio: "inherit"
    });
    child.procwatchIndex = index;
    child.procwatchStopping = false;
    child.procwatchExited = false;
    children.add(child);
    child.on("exit", () => {
      child.procwatchExited = true;
      children.delete(child);
      if (!shuttingDown && !child.procwatchStopping && children.size < targetInstances) start(index);
    });
    return child;
  }

  function stopChild(child) {
    child.procwatchStopping = true;
    child.kill("SIGTERM");
    setTimeout(() => {
      if (!child.procwatchExited) child.kill("SIGKILL");
    }, 5000).unref();
  }

  function waitChildExit(child) {
    return new Promise((resolve) => {
      if (!child || child.procwatchExited) {
        resolve();
        return;
      }
      child.once("exit", resolve);
      setTimeout(resolve, 6000).unref();
    });
  }

  async function scaleTo(count) {
    targetInstances = Math.max(1, Number(count || 1));
    while (children.size < targetInstances) start();
    if (children.size > targetInstances) {
      const excess = [...children].slice(targetInstances);
      for (const child of excess) stopChild(child);
      await Promise.all(excess.map(waitChildExit));
    }
    return { workers: children.size, target: targetInstances };
  }

  async function reloadWorkers() {
    const current = [...children];
    for (const child of current) {
      start(child.procwatchIndex);
      await delay(100);
      stopChild(child);
      await waitChildExit(child);
    }
    return { workers: children.size, target: targetInstances };
  }

  for (let i = 0; i < targetInstances; i += 1) start(i);
  nextIndex = targetInstances;

  startControlServer(async (request) => {
    if (request.type === "scale") return scaleTo(request.instances);
    if (request.type === "reload") return reloadWorkers();
    throw new Error(`unsupported cluster control request: ${request.type}`);
  });

  installShutdown(() => {
    for (const child of children) stopChild(child);
  });
}
