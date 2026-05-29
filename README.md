# Promon

Promon is a Rust-first Node.js process manager for JavaScript and TypeScript projects.

This repository currently contains the first MVP implementation:

- `promon init`
- `promon validate`
- `promon doctor`
- `promon start`
- `promon stop`
- `promon list`

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
