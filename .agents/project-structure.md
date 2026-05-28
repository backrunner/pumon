# Promon Project Structure

## Proposed Repository Layout

```text
promon/
  .agents/
    README.md
    requirements.md
    development-plan.md
    module-design.md
    project-structure.md
    release-engineering.md
  .github/
    workflows/
      ci.yml
      release.yml
      npm.yml
  crates/
    promon-cli/
      src/
        main.rs
        commands/
        output/
    promon-core/
      src/
        app.rs
        error.rs
        lib.rs
        restart.rs
        spec.rs
        status.rs
    promon-config/
      src/
        detect.rs
        formats/
        lib.rs
        normalize.rs
        validate.rs
    promon-daemon/
      src/
        handlers/
        lib.rs
        reconcile.rs
        runtime.rs
        state/
    promon-ipc/
      src/
        client.rs
        lib.rs
        protocol.rs
        server.rs
        transport/
    promon-process/
      src/
        child.rs
        cluster.rs
        command_plan.rs
        lib.rs
        supervisor.rs
        termination.rs
    promon-node-support/
      src/
        config_loader.rs
        lib.rs
        package_manager.rs
        runtime_resolver.rs
    promon-platform/
      src/
        lib.rs
        metrics.rs
        paths.rs
        unix.rs
        windows.rs
    promon-service/
      src/
        lib.rs
        linux.rs
        macos.rs
        windows.rs
    promon-logging/
      src/
        lib.rs
        rotate.rs
        tail.rs
        writer.rs
    promon-watch/
      src/
        debounce.rs
        filters.rs
        lib.rs
        manager.rs
    promon-scheduler/
      src/
        cron.rs
        lib.rs
        scheduler.rs
    promon-tui/
      src/
        app.rs
        lib.rs
        screens/
        widgets/
  packages/
    promon/
      package.json
      bin/
        promon.js
      scripts/
        install.js
        resolve-binary.js
      src/
        channel.ts
        downloader.ts
        platform.ts
    cluster-shim/
      package.json
      src/
        index.ts
        protocol.ts
        worker.ts
    node-support/
      package.json
      src/
        config-loader.ts
        package-json.ts
        protocol.ts
  examples/
    basic/
      ecosystem.config.json
      server.js
    typescript/
      ecosystem.config.ts
      package.json
      src/server.ts
      dist/server.js
    package-script/
      ecosystem.config.js
      package.json
      server.mjs
    cluster/
      ecosystem.config.yaml
      server.js
  fixtures/
    node-apps/
      crash/
      memory/
      logger/
      watcher/
      ts-loader/
      ts-prebuilt/
      package-script/
  docs/
    cli.md
    configuration.md
    service.md
    release-channels.md
  scripts/
    build-release.sh
    package-npm.mjs
  Cargo.toml
  README.md
  LICENSE
```

## Workspace Rules

- Keep Rust crates focused on one responsibility.
- Put cross-platform abstractions in `promon-platform`, not scattered through feature code.
- Keep daemon command handling separate from the lower-level supervisor.
- Keep CLI rendering separate from command behavior.
- Treat `packages/promon` as a binary installer/wrapper, not the core implementation.
- Treat `packages/cluster-shim` as Node cluster runtime glue, not a general Node SDK.
- Treat `packages/node-support` as bundled Node-side support code for config loading and JS/TS runtime glue.

## File Complexity Guidelines

Targets:

- Rust source files: usually under 300-400 lines.
- CLI command modules: one command group per file.
- Daemon handlers: one operation group per file.
- Tests: split by behavior rather than creating one huge integration file.

Acceptable exceptions:

- Generated code.
- Platform FFI bindings.
- Snapshot fixtures.

When a file grows:

- Split by responsibility before adding large comments.
- Prefer clear domain modules over generic `utils`.
- Move parsing, validation, and normalization into separate functions or modules.

## Crate Dependency Direction

Recommended dependency direction:

```text
promon-cli       -> promon-core, promon-config, promon-ipc
promon-tui       -> promon-core, promon-ipc
promon-daemon    -> all core runtime crates
promon-process   -> promon-core, promon-platform, promon-logging, promon-node-support
promon-config    -> promon-core, promon-platform, promon-node-support
promon-node-support -> promon-core, promon-platform
promon-service   -> promon-core, promon-platform
promon-ipc       -> promon-core
promon-logging   -> promon-core, promon-platform
promon-watch     -> promon-core
promon-scheduler -> promon-core
promon-platform  -> promon-core
promon-core      -> minimal third-party dependencies
```

Avoid dependency cycles by keeping shared types in `promon-core`.

## Recommended Rust Dependencies

Core:

- `anyhow` for application-level errors where appropriate.
- `thiserror` for library errors.
- `serde`, `serde_json`, `toml`, `serde_yaml` or `serde_yml`.
- `tokio` for async process, IPC, timers, and IO.
- `tracing`, `tracing-subscriber`.
- `clap`.
- `uuid`.
- `time`.

Process/platform:

- `sysinfo` for process metrics where sufficient.
- `nix` for Unix process behavior.
- `windows-service` for Windows services.
- `directories` for platform path conventions.

Config/watch/schedule:

- `notify`.
- `globset`.
- `cron` or `croner`.

Node.js project support:

- Avoid adding a full JavaScript engine to Rust for config execution.
- Prefer invoking the configured Node.js runtime through a bundled config loader.
- Use Rust for schema validation after JavaScript/TypeScript config output is converted to JSON.

TUI:

- `ratatui`.
- `crossterm`.

Persistence:

- `sqlx` with SQLite or `rusqlite`.

Release:

- `cargo-dist` may be considered for binary packaging, but the final flow must still support stable/beta/alpha channels and npm wrapper resolution.

## NPM Package Layout

`packages/promon` should provide:

- A `bin` entry named `promon`.
- An install script that downloads the correct binary.
- A runtime fallback that downloads on first run if install scripts were skipped.
- Channel selection through package dist-tags and environment variables.

The wrapper should:

- Detect OS and CPU architecture.
- Resolve the desired release channel.
- Download from GitHub Releases.
- Verify checksum.
- Cache the binary under the package or user cache directory.
- Exec the binary with forwarded arguments.

## Cluster Shim Layout

`packages/cluster-shim` should provide:

- A TypeScript source package compiled to JavaScript.
- A small protocol for Rust-to-Node control messages.
- Worker lifecycle reporting.
- Graceful reload behavior.

The shim should not become a public API unless a future product decision explicitly expands Promon beyond CLI use.

## Node Support Package Layout

`packages/node-support` should provide bundled JavaScript built from TypeScript:

- `config-loader`: Loads `ecosystem.config.js`, `.cjs`, `.mjs`, `.ts`, `.mts`, and `.cts`.
- `package-json`: Reads package metadata and package scripts.
- Shared protocol helpers for structured JSON output and errors.

This package is internal runtime support. It should be versioned and bundled with Promon releases rather than installed into user projects.

## Documentation Layout

Docs should remain user-facing and concise:

- `docs/cli.md`: Commands and examples.
- `docs/configuration.md`: Ecosystem fields.
- `docs/service.md`: System service install details by OS.
- `docs/release-channels.md`: Stable/beta/alpha behavior.

Deep implementation details belong in `.agents` or crate-level rustdoc.
