# Configuration

Procwatch supports:

- `ecosystem.config.js`
- `ecosystem.config.cjs`
- `ecosystem.config.mjs`
- `ecosystem.config.ts`
- `ecosystem.config.mts`
- `ecosystem.config.cts`
- `ecosystem.config.json`
- `ecosystem.config.toml`
- `ecosystem.config.yaml`
- `ecosystem.config.yml`

Core fields: `name`, `script`, `command`, `cwd`, `args`, `node_args`, `interpreter`, `interpreter_args`, `package_manager`, `package_script`, `env`, `exec_mode`, `instances`, `watch`, `restart`, `max_memory_restart`, `cron_restart`, and `log`.

`watch` accepts either a boolean or an object. Object form supports `enabled`, `paths`, `include`, `ignore`, `debounce_ms`, and `reload`. `ignore_watch` is also accepted as a top-level PM2-style alias and is merged into `watch.ignore`.

`log.max_size_bytes` enables log rotation for stdout and stderr; `log.retain` controls how many rotated files are kept. Runtime rotation is active when Procwatch remains attached as supervisor, such as `daemon`, `watch`, or `start --wait`. Direct detached `start` still performs startup-time rotation. `log.merge: true` writes stderr to the stdout log path.
