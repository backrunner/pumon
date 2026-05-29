# Procwatch Development Plan

## Phase 0: Repository Foundation

Goals:

- Create the Rust workspace and npm package skeleton.
- Establish formatting, linting, testing, and release metadata.
- Decide the initial state directory layout.
- Add sample ecosystem configs.
- Add fixture JavaScript and TypeScript apps that represent the first-batch support matrix.

Deliverables:

- `Cargo.toml` workspace.
- Core crates under `crates/`.
- npm wrapper under `packages/procwatch/`.
- GitHub Actions workflow skeleton.
- Basic `procwatch --version` and `procwatch doctor`.
- JS/TS fixtures covering CommonJS, ESM, TS via loader, TS prebuilt output, and package manager scripts.

Recommended crates:

- `procwatch-cli`
- `procwatch-core`
- `procwatch-daemon`
- `procwatch-config`
- `procwatch-process`
- `procwatch-ipc`
- `procwatch-service`
- `procwatch-logging`
- `procwatch-tui`
- `procwatch-platform`

## Phase 1: JavaScript/TypeScript Config and Direct Process Supervision

Goals:

- Parse JavaScript, TypeScript, JSON, TOML, and YAML ecosystem configs.
- Resolve app specs into normalized runtime specs.
- Start and stop JavaScript and TypeScript Node.js apps in fork mode.
- Support direct script entrypoints and package-manager script starts.
- Validate Node.js, package manager, local runtime loader, and entrypoint availability.
- Capture stdout and stderr to logs.
- Maintain in-memory process state.

Deliverables:

- `procwatch init`.
- `procwatch validate`.
- `procwatch start`.
- `procwatch stop`.
- `procwatch list`.
- JavaScript/TypeScript ecosystem config loader.
- Runtime command resolver for `node`, local `node_modules/.bin`, npm, pnpm, Yarn, and Bun.
- Log file creation.
- Unit tests for config parsing and runtime spec normalization.

Exit criteria:

- Basic CommonJS, ESM, TypeScript-loader, TypeScript-prebuilt, and package-script HTTP servers can be started, listed, stopped, and inspected on macOS/Linux/Windows.
- Invalid configs produce actionable errors.
- `procwatch doctor` identifies missing Node.js, missing package managers, missing local TS loaders, and missing package scripts.

## Phase 2: Daemon and IPC

Goals:

- Move process supervision into a long-running daemon.
- Build local IPC for CLI-to-daemon communication.
- Persist desired app state.
- Add daemon lifecycle commands.

Deliverables:

- `procwatch daemon start|stop|status`.
- IPC request/response schema.
- State persistence.
- CLI auto-connect behavior.
- Daemon crash recovery basics.

Exit criteria:

- CLI commands work through the daemon.
- Restarting the CLI does not lose managed process state.
- Restarting the daemon reconciles desired apps and actual processes.

## Phase 3: Restart Policies and Health Controls

Goals:

- Add crash restart and backoff.
- Add memory threshold restart.
- Add scheduled restart.
- Track restart counters and unstable states.

Deliverables:

- Restart policy engine.
- Platform memory metrics.
- Cron or interval scheduler.
- Integration tests with intentionally crashing Node scripts.
- Integration tests must include both JavaScript and TypeScript crash fixtures.

Exit criteria:

- Crashed apps are restarted according to policy.
- Memory limit and scheduled restart can be verified with test apps.
- Crash loops are throttled and visible in status.

## Phase 4: Logs and Log Rotation

Goals:

- Provide real-time log streaming.
- Implement rotation by size and retention.
- Support stdout/stderr/merged views.

Deliverables:

- `procwatch logs [app]`.
- Rotation worker or rotation-on-write strategy.
- Log metadata and cleanup.
- Tests for rotation boundaries.

Exit criteria:

- Long-running apps do not grow logs without bound when rotation is enabled.
- Users can stream logs while apps are running.

## Phase 5: Cluster Mode and Scaling

Goals:

- Support clustered Node.js apps.
- Add scale up/down.
- Add graceful reload.

Deliverables:

- Node cluster shim package.
- Rust supervisor integration for cluster master lifecycle.
- Cluster compatibility for JavaScript scripts, TypeScript loader execution, and prebuilt TypeScript output.
- `procwatch scale`.
- `procwatch reload`.
- Integration tests for multiple workers.

Exit criteria:

- `instances = 2` starts two Node workers behind a cluster master.
- Cluster mode works for JS and TS app entrypoints using the same runtime command resolver as fork mode.
- `instances = "max"` resolves correctly.
- Scaling changes the worker count without restarting the whole app when possible.

## Phase 6: Watch Mode

Goals:

- Watch file changes.
- Debounce restarts/reloads.
- Respect ignore patterns.

Deliverables:

- Watch configuration parser.
- Watch manager.
- Restart/reload integration.
- Tests for debounce and ignore patterns.

Exit criteria:

- Editing a watched file triggers exactly one restart/reload after debounce.
- Log and state directories are ignored by default.

## Phase 7: System Service Integration

Goals:

- Register Procwatch daemon as a system startup service.
- Support Linux, macOS, and Windows service backends.

Deliverables:

- `procwatch service install|uninstall|start|stop|status`.
- systemd backend.
- launchd backend.
- Windows Service backend.
- Platform-specific docs.

Exit criteria:

- Procwatch can start on boot/login and restore desired apps on each supported OS.
- Installation failures have clear rollback and diagnostics.

## Phase 8: TUI

Goals:

- Build an interactive terminal UI backed by daemon IPC.
- Manage apps and inspect logs from one screen.

Deliverables:

- `procwatch tui`.
- Process list.
- Detail panel.
- Logs panel.
- Actions for start/stop/restart/reload/scale.

Exit criteria:

- Common operations can be completed without leaving the TUI.
- TUI updates live without excessive CPU usage.

## Phase 9: Release and Distribution Hardening

Goals:

- Publish binaries through GitHub Releases.
- Publish npm wrapper package for `npx procwatch`.
- Separate stable, beta, and alpha channels.

Deliverables:

- Release workflows.
- npm package install/run scripts.
- Binary download cache.
- Checksums and optional signatures.
- Version/channel docs.

Exit criteria:

- Users can run stable through `npx procwatch`.
- Users can opt into beta and alpha.
- GitHub Releases contain platform-specific artifacts and checksums.

## Test Strategy

Unit tests:

- Config parsing.
- JavaScript/TypeScript config loader behavior.
- Runtime command resolution for JS, TS, and package scripts.
- Restart policy decisions.
- Backoff calculations.
- App spec normalization.
- Log rotation naming and retention.
- IPC schema serialization.

Integration tests:

- Start/stop real JavaScript and TypeScript Node.js scripts.
- Run package manager script fixtures.
- Run TS fixtures through `tsx` or `ts-node` and through prebuilt `dist` output.
- Crash and restart behavior.
- Memory restart using a controlled allocator script.
- Log streaming and rotation.
- Watch mode restart.
- Daemon lifecycle.

Platform tests:

- Linux CI for systemd unit generation and non-privileged behavior.
- macOS CI for launchd plist generation.
- Windows CI for service manifest/registration dry-run and named pipe IPC.

Manual release checks:

- Fresh install via `npx`.
- Direct binary download.
- Upgrade from previous version.
- Service install and uninstall on each OS.

## Key Risks

- Cross-platform service behavior has permission and lifecycle differences.
- Node cluster load balancing is platform-sensitive.
- Windows signal handling differs from Unix process control.
- Log rotation can race with live writes and log tailing.
- JavaScript/TypeScript ecosystem configs require executing user-controlled config code.
- TypeScript runtime support varies by loader, module system, and package manager.

## Risk Responses

- Hide platform differences behind narrow traits and test generated service artifacts.
- Use a Node-side cluster shim for real cluster semantics.
- Implement graceful shutdown as a platform abstraction, with fallback force-kill.
- Centralize log writes through one writer per stream where possible.
- Execute JavaScript/TypeScript ecosystem configs only through a constrained loader process, then validate the resulting JSON in Rust.
- Treat TypeScript execution as explicit runtime resolution: Procwatch supports configured loaders and prebuilt output, but does not compile projects automatically.
