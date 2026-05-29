#!/usr/bin/env bash
set -euo pipefail

cargo build -p procwatch-cli

PROCWATCH_BIN="${PROCWATCH_BIN:-target/debug/procwatch}"
"$PROCWATCH_BIN" --version
"$PROCWATCH_BIN" tui --help
"$PROCWATCH_BIN" doctor
"$PROCWATCH_BIN" prune
"$PROCWATCH_BIN" validate examples/basic/ecosystem.config.json
"$PROCWATCH_BIN" validate examples/typescript/ecosystem.config.ts
"$PROCWATCH_BIN" validate examples/package-script/ecosystem.config.js
"$PROCWATCH_BIN" validate examples/cluster/ecosystem.config.json
"$PROCWATCH_BIN" validate fixtures/node-apps/ts-prebuilt/ecosystem.config.json
"$PROCWATCH_BIN" validate fixtures/node-apps/package-script/ecosystem.config.js
"$PROCWATCH_BIN" validate fixtures/node-apps/crash/ecosystem.config.json
"$PROCWATCH_BIN" validate fixtures/node-apps/scheduled/ecosystem.config.json
"$PROCWATCH_BIN" validate fixtures/node-apps/watcher/ecosystem.config.json
"$PROCWATCH_BIN" validate fixtures/node-apps/log-rotate/ecosystem.config.json
"$PROCWATCH_BIN" validate fixtures/node-apps/foreground-multi/ecosystem.config.json
"$PROCWATCH_BIN" service status

tmp_home="$(mktemp -d /tmp/procwatch-smoke.XXXXXX)"
watch_pid=""
foreground_wait_pid=""
trap 'if [ -n "${watch_pid:-}" ]; then kill "$watch_pid" >/dev/null 2>&1 || true; fi; if [ -n "${foreground_wait_pid:-}" ]; then kill "$foreground_wait_pid" >/dev/null 2>&1 || true; fi; PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" daemon stop >/dev/null 2>&1 || true; PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" stop all >/dev/null 2>&1 || true; rm -rf "$tmp_home"' EXIT

PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" start examples/basic/ecosystem.config.json
sleep 1
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" list
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" status basic-js
status_json="$(PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" --json status basic-js)"
node -e 'const r = JSON.parse(process.argv[1]); if (r.count !== 1 || r.processes[0].name !== "basic-js") process.exit(1);' "$status_json"
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" logs basic-js -n 5
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" reload examples/basic/ecosystem.config.json
sleep 1
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" restart examples/basic/ecosystem.config.json
sleep 1
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" stop basic-js
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" start examples/basic/server.js
sleep 1
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" stop server
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" start examples/basic/server.js
sleep 1
server_json="$(PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" --json list)"
server_pid="$(node -e 'const r = JSON.parse(process.argv[1]); const p = r.processes.find((item) => item.name === "server"); if (!p) process.exit(1); console.log(p.pid);' "$server_json")"
kill -9 "$server_pid" >/dev/null 2>&1 || true
sleep 1
stale_json="$(PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" --json list)"
node -e 'const r = JSON.parse(process.argv[1]); const p = r.processes.find((item) => item.name === "server"); if (!p || p.status !== "unknown") process.exit(1);' "$stale_json"
prune_json="$(PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" --json prune)"
node -e 'const r = JSON.parse(process.argv[1]); if (r.count !== 1 || r.removed[0].name !== "server") process.exit(1);' "$prune_json"
empty_after_prune="$(PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" --json list)"
node -e 'const r = JSON.parse(process.argv[1]); if (r.processes.length !== 0) process.exit(1);' "$empty_after_prune"
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" start --wait fixtures/node-apps/foreground-multi/ecosystem.config.json >"$tmp_home/foreground-wait.log" 2>&1 &
foreground_wait_pid=$!
foreground_ready=""
for _ in $(seq 1 30); do
  foreground_list="$(PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" --json list)"
  if node -e 'const r = JSON.parse(process.argv[1]); const ps = r.processes || r.payload?.processes || []; const names = new Set(ps.map((p) => p.name)); process.exit(names.has("foreground-one") && names.has("foreground-two") ? 0 : 1);' "$foreground_list"; then
    foreground_ready=1
    break
  fi
  sleep 0.2
done
test "$foreground_ready" = "1"
if [ "${CI:-}" = "true" ]; then
  kill -KILL "$foreground_wait_pid" >/dev/null 2>&1 || true
  wait "$foreground_wait_pid" || true
  foreground_wait_pid=""
  PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" stop all
else
  kill -TERM "$foreground_wait_pid"
  for _ in $(seq 1 30); do
    if ! kill -0 "$foreground_wait_pid" >/dev/null 2>&1; then
      break
    fi
    sleep 0.2
  done
  if kill -0 "$foreground_wait_pid" >/dev/null 2>&1; then
    kill -KILL "$foreground_wait_pid" >/dev/null 2>&1 || true
  fi
  wait "$foreground_wait_pid" || true
  foreground_wait_pid=""
fi
foreground_after="$(PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" --json list)"
node -e 'const r = JSON.parse(process.argv[1]); const ps = r.processes || r.payload?.processes || []; if (ps.length !== 0) process.exit(1);' "$foreground_after"
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" start examples/cluster/ecosystem.config.json
sleep 1
cluster_before="$(PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" --json list)"
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" scale examples/cluster/ecosystem.config.json 1
sleep 1
cluster_after_scale="$(PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" --json list)"
node -e 'const before = JSON.parse(process.argv[1]).processes.find((p) => p.name === "cluster-js"); const after = JSON.parse(process.argv[2]).processes.find((p) => p.name === "cluster-js"); if (!before || !after || before.pid !== after.pid) process.exit(1);' "$cluster_before" "$cluster_after_scale"
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" reload examples/cluster/ecosystem.config.json
sleep 1
cluster_after_reload="$(PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" --json list)"
node -e 'const before = JSON.parse(process.argv[1]).processes.find((p) => p.name === "cluster-js"); const after = JSON.parse(process.argv[2]).processes.find((p) => p.name === "cluster-js"); if (!before || !after || before.pid !== after.pid) process.exit(1);' "$cluster_after_scale" "$cluster_after_reload"
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" stop cluster-js
watch_dir="$tmp_home/watch-fixture"
mkdir -p "$watch_dir"
cp fixtures/node-apps/watcher/server.js "$watch_dir/server.js"
cp fixtures/node-apps/watcher/ecosystem.config.json "$watch_dir/ecosystem.config.json"
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" watch "$watch_dir/ecosystem.config.json" --interval-ms 100 >"$tmp_home/watch.log" 2>&1 &
watch_pid=$!
sleep 1
watch_before="$(PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" --json list)"
printf '\n// smoke change\n' >> "$watch_dir/server.js"
watch_restarted=""
for _ in $(seq 1 30); do
  watch_after="$(PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" --json list)"
  if node -e 'const before = JSON.parse(process.argv[1]).processes.find((p) => p.name === "watcher-fixture"); const after = JSON.parse(process.argv[2]).processes.find((p) => p.name === "watcher-fixture"); process.exit(before && after && before.pid !== after.pid ? 0 : 1);' "$watch_before" "$watch_after"; then
    watch_restarted=1
    break
  fi
  sleep 0.2
done
test "$watch_restarted" = "1"
set +e
kill "$watch_pid" >/dev/null 2>&1 || true
wait "$watch_pid" >/dev/null 2>&1 || true
set -e
watch_pid=""
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" stop watcher-fixture
HOME="$tmp_home" "$PROCWATCH_BIN" service install examples/basic/ecosystem.config.json
HOME="$tmp_home" "$PROCWATCH_BIN" service status
HOME="$tmp_home" "$PROCWATCH_BIN" service uninstall
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" daemon start examples/basic/ecosystem.config.json
sleep 1
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" daemon status
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" daemon ping
ping_json="$(PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" --json daemon ping)"
node -e 'const r = JSON.parse(process.argv[1]); if (r.version !== 1 || !r.request_id || !r.ok || r.payload.pong !== true) process.exit(1);' "$ping_json"
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" daemon list
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" list
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" status basic-js
daemon_status_json="$(PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" --json status basic-js)"
node -e 'const r = JSON.parse(process.argv[1]); if (r.count !== 1 || r.processes[0].name !== "basic-js") process.exit(1);' "$daemon_status_json"
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" reload examples/basic/ecosystem.config.json
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" start examples/cluster/ecosystem.config.json
sleep 1
daemon_cluster_before="$(PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" --json list)"
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" scale examples/cluster/ecosystem.config.json 1
sleep 1
daemon_cluster_after_scale="$(PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" --json list)"
node -e 'const before = JSON.parse(process.argv[1]).payload.processes.find((p) => p.name === "cluster-js"); const after = JSON.parse(process.argv[2]).payload.processes.find((p) => p.name === "cluster-js"); if (!before || !after || before.pid !== after.pid) process.exit(1);' "$daemon_cluster_before" "$daemon_cluster_after_scale"
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" reload examples/cluster/ecosystem.config.json
sleep 1
daemon_cluster_after_reload="$(PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" --json list)"
node -e 'const before = JSON.parse(process.argv[1]).payload.processes.find((p) => p.name === "cluster-js"); const after = JSON.parse(process.argv[2]).payload.processes.find((p) => p.name === "cluster-js"); if (!before || !after || before.pid !== after.pid) process.exit(1);' "$daemon_cluster_after_scale" "$daemon_cluster_after_reload"
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" stop cluster-js
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" start fixtures/node-apps/log-rotate/ecosystem.config.json
for _ in $(seq 1 30); do
  if [ -f "$tmp_home/logs/log-rotate-fixture/out.log.1" ]; then
    break
  fi
  sleep 0.2
done
test -f "$tmp_home/logs/log-rotate-fixture/out.log.1"
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" stop log-rotate-fixture
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" restart examples/basic/ecosystem.config.json
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" stop basic-js
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" start examples/basic/ecosystem.config.json
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" start examples/package-script/ecosystem.config.js
sleep 1
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" daemon stop
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" daemon start examples/basic/ecosystem.config.json
sleep 1
restored_json="$(PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" --json list)"
node -e 'const r = JSON.parse(process.argv[1]); const names = new Set(r.payload.processes.map((p) => p.name)); if (!names.has("basic-js") || !names.has("package-script")) process.exit(1);' "$restored_json"
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" stop all
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" start fixtures/node-apps/scheduled/ecosystem.config.json
scheduled_first="$(PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" --json list)"
sleep 4
scheduled_second="$(PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" --json list)"
node -e 'const first = JSON.parse(process.argv[1]).payload.processes.find((p) => p.name === "scheduled-fixture"); const second = JSON.parse(process.argv[2]).payload.processes.find((p) => p.name === "scheduled-fixture"); if (!first || !second || first.pid === second.pid) process.exit(1);' "$scheduled_first" "$scheduled_second"
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" stop all
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" daemon stop
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" daemon stop
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" list

set +e
PROCWATCH_HOME="$tmp_home" "$PROCWATCH_BIN" start --wait fixtures/node-apps/crash/ecosystem.config.json
crash_code=$?
set -e
test "$crash_code" -ne 0
