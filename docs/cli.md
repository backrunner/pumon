# Procwatch CLI

Implemented MVP commands:

- `procwatch init [output]`
- `procwatch validate [config]`
- `procwatch doctor [config]`
- `procwatch start [config|script]`
- `procwatch start --wait [config|script]`
- `procwatch stop <name|all>`
- `procwatch restart [config|script]`
- `procwatch reload [config|script]`
- `procwatch scale <config> <instances>`
- `procwatch status [name]`
- `procwatch prune`
- `procwatch logs [name] [-n lines] [--follow]`
- `procwatch watch [config|script]`
- `procwatch daemon start|stop|status|ping|list`
- `procwatch service install|start|stop|uninstall|status`
- `procwatch tui [config]`

`procwatch daemon start` launches `procwatch daemon run <config>`, keeps desired apps reconciled, and exposes local IPC. On Unix platforms IPC uses a Unix socket under `PROCWATCH_HOME/daemon`; on Windows it uses a localhost TCP listener address file.

When the daemon is reachable, normal management commands such as `start`, `stop`, `restart`, `reload`, `scale`, `status`, and `list` route through daemon IPC and update daemon desired state where applicable. If no daemon is reachable, they fall back to direct local process management.

`procwatch prune` removes stale process records whose PID is no longer alive. It does not change daemon desired state.

For cluster apps, `scale` and `reload` use the cluster shim control channel when the app is already running, so the cluster master process can stay alive while workers are resized or replaced. The control channel is loopback-only, pid-checked, and token-checked through the local control address file. Non-cluster apps still use the supervisor restart path.

`procwatch watch` honors `watch.paths`, `watch.include`, `watch.ignore`, `ignore_watch`, `watch.debounce_ms`, and `watch.reload`. If no app has watch enabled, the explicit `watch` command watches all resolved apps.

`procwatch start --wait` supervises all resolved apps concurrently in the foreground, keeps them visible to `procwatch list` and `procwatch status`, and shuts them down cleanly on `Ctrl+C`.

`procwatch tui [config]` opens an interactive terminal manager. It can list managed processes, tail logs, stop a selected process, and, when a config is loaded, start all apps or start/restart/reload/scale the selected app.
