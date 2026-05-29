# Procwatch Planning Documents

Procwatch is a Rust-first Node.js process manager intended to provide PM2-like operational behavior across Windows, macOS, and Linux. This directory contains the product and engineering planning documents that should guide implementation.

## Documents

- [requirements.md](requirements.md): Product requirements, supported workflows, and non-goals.
- [development-plan.md](development-plan.md): Phased delivery plan, milestones, testing strategy, and risk handling.
- [module-design.md](module-design.md): Runtime architecture, process model, daemon/service model, clustering, logging, TUI, and persistence design.
- [project-structure.md](project-structure.md): Proposed repository layout, crate boundaries, package organization, and file complexity guidelines.
- [release-engineering.md](release-engineering.md): GitHub release channels, binary distribution, npm/npx wrapper behavior, CI/CD, and versioning.

## Core Principles

1. Rust owns the core supervisor, daemon, service integration, process lifecycle, configuration parsing, logging, and TUI.
2. JavaScript and TypeScript Node.js projects are both first-batch supported targets.
3. TypeScript is allowed where it materially improves Node.js-specific behavior, especially cluster bootstrap, TypeScript entrypoint support, JavaScript/TypeScript config loading, and npm distribution glue.
4. Procwatch initially supports Node.js applications only.
5. The CLI is the only public interface for now. No Node programming API is planned for the initial product.
6. Cross-platform behavior must be designed up front rather than patched in per operating system.
7. Operational safety matters: process state, logs, restart policies, and service installation should be deterministic and inspectable.
