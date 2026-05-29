# Procwatch

Procwatch is a Rust-first Node.js process manager for JavaScript and TypeScript projects.

This repository currently contains the first MVP implementation:

- `procwatch init`
- `procwatch validate`
- `procwatch doctor`
- `procwatch prune`
- `procwatch start`
- `procwatch start --wait`
- `procwatch stop`
- `procwatch restart`
- `procwatch reload`
- `procwatch scale`
- `procwatch status`
- `procwatch list`
- `procwatch logs`
- `procwatch watch`
- `procwatch daemon`
- `procwatch service`
- `procwatch tui`

Current implemented surface:

- JavaScript/TypeScript ecosystem config loading.
- Fork mode process start/stop/restart/list.
- Foreground supervision with restart, memory threshold, and interval restart.
- Cluster mode through the bundled Node cluster shim.
- Log capture, tailing, follow mode, and size-based rotation.
- Polling watch mode.
- User-level service definition generation.
- Background daemon wrapper for `start --wait`.
- Minimal terminal process dashboard.
- GitHub Release and npm wrapper scaffolding.
- Stale process pruning.
