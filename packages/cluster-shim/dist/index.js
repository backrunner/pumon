import cluster from "node:cluster";
import { spawn } from "node:child_process";
import os from "node:os";

function help() {
  console.log("usage: promon-cluster-shim '<json-spec>'");
}

const raw = process.argv[2];
if (!raw || raw === "--help") {
  help();
  process.exit(0);
}

const spec = JSON.parse(raw);
const instances = spec.instances === "max" ? os.availableParallelism() : Math.max(1, Number(spec.instances || 1));
const worker = spec.worker;

if ((worker.interpreter || "node") === "node") {
  if (cluster.isPrimary) {
    cluster.setupPrimary({
      exec: worker.script,
      args: worker.args || [],
      execArgv: [...(worker.nodeArgs || []), ...(worker.interpreterArgs || [])],
      cwd: worker.cwd
    });
    for (let i = 0; i < instances; i += 1) {
      cluster.fork({ ...process.env, ...(worker.env || {}), PROMON_WORKER_ID: String(i) });
    }
    cluster.on("exit", (_worker, code) => {
      if (code !== 0) {
        cluster.fork({ ...process.env, ...(worker.env || {}) });
      }
    });
  }
} else {
  const children = new Set();
  function start(index) {
    const child = spawn(worker.interpreter, [...(worker.interpreterArgs || []), worker.script, ...(worker.args || [])], {
      cwd: worker.cwd,
      env: { ...process.env, ...(worker.env || {}), PROMON_WORKER_ID: String(index) },
      stdio: "inherit"
    });
    children.add(child);
    child.on("exit", (code) => {
      children.delete(child);
      if (code !== 0) start(index);
    });
  }
  for (let i = 0; i < instances; i += 1) start(i);
  process.on("SIGTERM", () => {
    for (const child of children) child.kill("SIGTERM");
    process.exit(0);
  });
}

