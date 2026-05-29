# Promon CLI

Implemented MVP commands:

- `promon init [output]`
- `promon validate [config]`
- `promon doctor`
- `promon start [config|script]`
- `promon start --wait [config|script]`
- `promon stop <name|all>`
- `promon restart [config|script]`
- `promon scale <config> <instances>`
- `promon logs [name] [-n lines] [--follow]`
- `promon watch [config|script]`
- `promon daemon start|stop|status|ping|list`
- `promon service install|start|stop|uninstall|status`
- `promon tui`

`promon daemon start` launches `promon daemon run <config>`, keeps desired apps reconciled, and exposes local IPC. On Unix platforms IPC uses a Unix socket under `PROMON_HOME/daemon`; on Windows it uses a localhost TCP listener address file.
