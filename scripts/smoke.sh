#!/usr/bin/env bash
set -euo pipefail

cargo build -p promon-cli

PROMON_BIN="${PROMON_BIN:-target/debug/promon}"
"$PROMON_BIN" --version
"$PROMON_BIN" doctor
"$PROMON_BIN" validate examples/basic/ecosystem.config.json
"$PROMON_BIN" validate examples/typescript/ecosystem.config.ts
"$PROMON_BIN" validate examples/package-script/ecosystem.config.js
"$PROMON_BIN" validate fixtures/node-apps/ts-prebuilt/ecosystem.config.json
"$PROMON_BIN" validate fixtures/node-apps/package-script/ecosystem.config.js

tmp_home="$(mktemp -d /tmp/promon-smoke.XXXXXX)"
trap 'PROMON_HOME="$tmp_home" "$PROMON_BIN" stop basic-js >/dev/null 2>&1 || true; rm -rf "$tmp_home"' EXIT

PROMON_HOME="$tmp_home" "$PROMON_BIN" start examples/basic/ecosystem.config.json
sleep 1
PROMON_HOME="$tmp_home" "$PROMON_BIN" list
PROMON_HOME="$tmp_home" "$PROMON_BIN" stop basic-js

