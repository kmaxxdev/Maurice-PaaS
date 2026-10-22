# ADR-001: Rust for the control plane and the agent

- **Status:** accepted
- **Date:** 2026-10-22
- **Issue:** #2

## Context

The control plane (API, data model, Quadlet generation, plan/apply, UI hosting) and the agent (privileged operations on the host) are the only parts of Maurice written from scratch. The Podman ecosystem is Go: libpod bindings, the Quadlet parser and the reference panels (Rookery, quadlet_manager) are Go code that could be reused directly. The alternative is Rust, where Podman is reached through its REST API over a Unix socket and systemd through D-Bus.

## Decision

Rust, end to end, as one Cargo workspace. The control plane and the agent are separate crates compiled into one binary for Alpha; splitting into two processes later needs no rewrite.

Core choices inside the Rust stack:

- `tokio` + `axum` for the server; the Solid SPA is embedded with `rust-embed`.
- Podman via its REST API over `/run/user/<uid>/podman/podman.sock`; the client is generated from Podman's published OpenAPI specification, not hand-written.
- systemd via `zbus` (D-Bus), including `systemd --user` of workspace users through `--machine`.
- Quadlet units are generated as INI text from typed structs; syntax is verified with `podman quadlet` in dry-run mode before apply. Drift detection parses units back with an INI parser and compares.
- `sqlx` with compile-time checked queries against PostgreSQL (see [[004-postgresql-from-day-one]]).
- Static musl build so one file installs on any Linux host.

## Alternatives considered

- **Go.** Reuses libpod bindings, the upstream Quadlet parser and code from Rookery. Faster Alpha. Rejected because the project owner wants the stronger type system and memory safety for a process that runs as root, and accepts the longer Alpha.
- **NestJS / TypeScript.** Fastest CRUD and shared stack with the UI, but the agent would still be a native binary, giving two stacks from day one.

## Consequences

- Plan/apply can be enforced by types: `apply` accepts only a `ValidatedPlan` that the validator alone can construct.
- Exhaustive enums for service kind, deployment status and token scopes catch unhandled states at compile time.
- The Podman client and Quadlet glue are our code and must track Podman API versions.
- Expect Alpha to take roughly a third to a half longer than a Go implementation. Rookery and quadlet_manager remain architecture references only.
