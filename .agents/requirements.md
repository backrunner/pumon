# Promon Requirements

## Product Summary

Promon is a cross-platform Node.js process manager written primarily in Rust. It runs as a CLI and can also install a long-running Promon daemon as a system service. Users define JavaScript or TypeScript Node.js applications in an ecosystem-style configuration file, then use Promon to start, stop, restart, reload, scale, monitor, and inspect those applications.

Promon is inspired by PM2 but is not required to be command-compatible in the first release. Compatibility should focus on familiar concepts: ecosystem files, managed process lists, cluster mode, restart policies, logs, watch mode, and service startup.

## Target Users

- Developers running Node.js services locally across macOS, Linux, and Windows.
- Operators managing one or more Node.js applications on a server.
- Open-source users who expect simple installation through `npx` or direct binary download.
- Teams that need a small, readable, MIT-licensed process manager without a Node.js runtime dependency for the supervisor itself.

## First-Batch Node.js Project Support

Promon must fully support common JavaScript and TypeScript Node.js projects in the first public release. TypeScript support is not a later enhancement.

Required project types:

- Plain JavaScript projects using CommonJS.
- Plain JavaScript projects using native ES modules.
- TypeScript projects that run through a runtime loader such as `tsx`, `ts-node`, or a custom Node loader.
- TypeScript projects that build to JavaScript and run a compiled output file such as `dist/server.js`.
- Projects started through package manager scripts such as `npm run start`, `pnpm start`, `yarn start`, or `bun run start`, while still treating Node.js application management as the product boundary.
- Monorepo packages where each app has its own `cwd`.

Required runtime resolution:

- Detect and validate the configured Node.js executable.
- Support explicit `interpreter`, defaulting to `node`.
- Support `interpreter_args` or `node_args` for loaders such as `--loader ts-node/esm`, `--import tsx`, and source-map support.
- Support `script` entrypoints for `.js`, `.mjs`, `.cjs`, `.ts`, `.mts`, and `.cts`.
- Support command-style starts through `command` plus `args` for package manager scripts.
- Preserve `NODE_ENV` and named environment overlays.
- Resolve local binaries from `node_modules/.bin` before falling back to `PATH` when configured.

Required package manager awareness:

- Detect `packageManager` from `package.json` where helpful.
- Support npm, pnpm, Yarn, and Bun command invocation for Node.js project scripts.
- Do not install dependencies automatically unless a future explicit command is added.
- `promon doctor` must report missing Node.js, missing package manager, missing script, or missing local runtime loader.

Required TypeScript expectations:

- Promon does not compile TypeScript by default.
- Promon must allow users to run TypeScript directly through configured loaders/runners.
- Promon must allow users to run prebuilt TypeScript output.
- Error messages must distinguish Promon configuration failures from application/runtime loader failures.

## Supported Platforms

Promon must support:

- Linux x86_64 and arm64.
- macOS x86_64 and arm64.
- Windows x86_64, with arm64 considered after the core release.

Platform-specific behavior must be isolated behind traits/modules so the rest of the application can stay portable.

## Primary CLI Workflows

The CLI should support these commands in the first major delivery:

- `promon init`: Create a sample ecosystem config.
- `promon start <script|config>`: Start one script or all apps in a config.
- `promon stop <app|id|all>`: Stop managed processes.
- `promon restart <app|id|all>`: Restart processes.
- `promon reload <app|id|all>`: Gracefully reload where possible.
- `promon scale <app> <instances>`: Increase or decrease cluster workers.
- `promon list`: Show managed apps and worker state.
- `promon status [app]`: Show detailed state.
- `promon logs [app]`: Stream stdout/stderr from managed processes.
- `promon tui`: Open the interactive terminal UI.
- `promon daemon start|stop|status`: Manage the user/session daemon.
- `promon service install|uninstall|start|stop|status`: Register Promon as a system service.
- `promon validate [config]`: Validate configuration without starting apps.
- `promon doctor`: Diagnose Node.js path, service permissions, log directories, and platform support.

Command names may evolve during implementation, but the workflows above are required.

## Ecosystem Configuration

Promon should support an ecosystem-style config file with JavaScript familiarity but Rust-friendly parsing.

Required file names:

- `ecosystem.config.js`
- `ecosystem.config.cjs`
- `ecosystem.config.mjs`
- `ecosystem.config.ts`
- `ecosystem.config.mts`
- `ecosystem.config.cts`
- `ecosystem.config.json`
- `ecosystem.config.toml`
- `ecosystem.config.yaml` or `ecosystem.config.yml`

Initial config support must include JavaScript and TypeScript ecosystem files. Promon may implement this by invoking Node.js in a constrained config loader that imports or requires the config, normalizes it to JSON, and sends it back to Rust for validation.

Config loading requirements:

- Support CommonJS `module.exports`.
- Support ESM `export default`.
- Support TypeScript config loading through the same supported runtime loader strategy used for TS apps.
- Support configs exporting an array or an object containing `apps`.
- Keep all schema validation in Rust after JavaScript/TypeScript config evaluation.
- Provide deterministic error output when config execution fails.

Required app fields:

- `name`: Stable app name.
- `script`: Node.js entry file.
- `command`: Package manager or custom command for script-based starts.
- `cwd`: Working directory. Defaults to config file directory.
- `args`: Application arguments.
- `node_args`: Node.js runtime arguments.
- `interpreter`: Executable used to launch the app. Defaults to `node`.
- `interpreter_args`: Arguments passed before the script when using a custom interpreter or TS loader.
- `package_manager`: Optional override for npm, pnpm, yarn, or bun.
- `package_script`: Optional package script name such as `start`.
- `env`: Base environment variables.
- `env_production`, `env_development`, etc.: Named environment overlays.
- `instances`: Number of instances, `"max"`, or omitted.
- `exec_mode`: `"fork"` or `"cluster"`.
- `watch`: Boolean or watch configuration object.
- `ignore_watch`: Paths or globs excluded from watch mode.
- `restart`: Restart policy object.
- `max_memory_restart`: Memory threshold such as `512M`, `1G`.
- `cron_restart`: Cron expression or fixed interval for scheduled restarts.
- `log`: Log configuration object.

Required Promon-level fields:

- `promon.home`: State directory override.
- `promon.daemon`: Daemon behavior.
- `promon.log_rotate`: Default log rotation policy.
- `promon.node_path`: Node executable override.

## Process Lifecycle

Promon must:

- Spawn Node.js child processes with deterministic working directory, environment, arguments, and stdio capture.
- Persist desired app state so the daemon can reconcile actual state after restart.
- Detect process exits and classify them as expected stop, crash, failed start, or reload completion.
- Restart crashed processes according to policy.
- Support exponential backoff to avoid crash loops.
- Provide configurable limits for restart count and unstable startup windows.
- Support graceful shutdown before force killing.
- Support graceful reload for cluster workers.

## System Service and Daemon Support

Promon must support registering a daemon that starts automatically on system boot or user login.

Required platform integrations:

- Linux: systemd user service first; system-level systemd service where permissions allow.
- macOS: launchd user agent first; system launch daemon where permissions allow.
- Windows: Windows Service through a Rust service integration layer.

Service installation must:

- Generate predictable service files/manifests.
- Store service metadata in Promon state.
- Validate paths and permissions before writing service definitions.
- Provide clear rollback behavior if installation fails.
- Avoid hiding platform-specific permission requirements.

## Cluster Mode

Promon must support multiple Node.js worker processes for one app.

Required behavior:

- `instances` launches the desired number of workers.
- `instances = "max"` resolves to available logical CPU count.
- Scaling up starts new workers without stopping healthy workers.
- Scaling down gracefully drains or stops extra workers.
- Reload replaces workers gradually where possible.

Load balancing options:

1. Preferred: Promon launches a TypeScript/JavaScript cluster shim that uses Node.js `cluster` and lets Node own port sharing and round-robin behavior.
2. Alternative: Promon directly supervises N workers and relies on OS/socket behavior only where safe.

The initial stable cluster implementation should use a Node-side cluster shim because Node's cluster module already handles platform-specific balancing semantics.

## Restart Policies

Promon must support:

- Restart on crash.
- Restart after memory threshold is exceeded.
- Scheduled restart through cron-like expressions.
- Manual restart and reload.
- Backoff and max-restart thresholds.
- Optional `autorestart = false`.

Memory checks should be implemented by platform-specific process metrics, normalized behind one interface.

## Watch Mode

Promon must support watch mode for local development and controlled production reloads.

Required behavior:

- Watch app files from `cwd` by default.
- Support include and ignore globs.
- Debounce file change bursts.
- Restart or reload according to app mode and config.
- Avoid watching log and state directories by default.
- Handle symlinks conservatively and document behavior.

## Logs

Promon must centrally capture stdout and stderr for every managed process.

Required behavior:

- Separate stdout and stderr files per app or per worker.
- Optional merged log stream.
- `promon logs` follows logs in real time.
- Log entries include app name, worker id, stream, and timestamp when viewed through Promon.
- Built-in log rotation by size and retention count.
- Optional compression can be added after initial rotation is stable.

## TUI

Promon must provide a terminal UI for operational management.

Required views:

- Process list with app, mode, pid, status, uptime, restart count, CPU, memory.
- App detail panel.
- Live logs panel.
- Actions: start, stop, restart, reload, scale.
- Filter/search by app name.
- Keyboard-first navigation.

The TUI should be powered by the same daemon API as the CLI so behavior stays consistent.

## Persistence

Promon must store:

- App desired state.
- Runtime process state.
- Configuration snapshots or resolved app specs.
- Daemon PID/socket metadata.
- Service installation metadata.
- Restart history and crash counters.
- Log file metadata for rotation.

State format should be durable and inspectable. SQLite is recommended for daemon state once the design needs transactional updates; small JSON files are acceptable only for early prototypes.

## Daemon API

The CLI and TUI should communicate with the daemon through a local IPC API.

Recommended transports:

- Unix domain socket on Linux/macOS.
- Named pipe on Windows.

Requirements:

- Versioned request/response schema.
- Clear errors suitable for CLI display.
- Local-user access only by default.
- No remote network API in the initial release.

## Non-Goals For Initial Release

- Managing non-Node.js applications.
- Automatically compiling or bundling user TypeScript projects.
- Installing project dependencies automatically.
- Public Node.js programming API.
- Distributed process management.
- Remote dashboard.
- Container orchestration.
- Full PM2 command compatibility.
- Built-in metrics server.
- Cloud deployment integration.

## Quality Requirements

- Single files should remain small and cohesive; target under 400 lines, with exceptions only for generated code or dense platform bindings.
- All platform-specific code must live behind explicit modules and traits.
- Core process lifecycle logic must be unit tested.
- Integration tests should cover process spawning, crash restart, logs, config parsing, and IPC.
- CI must run formatting, linting, unit tests, and packaging checks.
- Public behavior must be documented before release.
