# ADR-002: One root agent with an operation whitelist

- **Status:** accepted
- **Date:** 2026-10-22
- **Issue:** #3

## Context

Every workspace is a separate system user running rootless Podman. Something has to create those users, write Quadlet units into their home, reload their `systemd --user` and talk to their Podman socket. Two models exist: one privileged agent that switches into users, or one unprivileged agent per workspace user.

## Decision

One agent process running as root, in the same binary as the control plane for Alpha. It switches into a workspace user per operation through systemd (`systemd-run --machine=<user>@` and D-Bus `--machine`), never through a shell.

The agent exposes a fixed set of operations and nothing else: create/delete workspace user, allocate/release subuid range, write/remove unit files, daemon-reload, start/stop/restart unit, read unit status, read journal, call the Podman API as the user. There is no "run command" operation.

## Alternatives considered

- **Agent per workspace user.** Smaller blast radius per process, but user creation and subuid allocation still need root, so a privileged component remains; N extra long-running processes to supervise and upgrade. Revisit at MVP if the whitelist proves insufficient.
- **sudoers-based access** (as in quadman). Works for a TUI, but a long-running service is better served by D-Bus and `machinectl` than by spawning `sudo`.

## Consequences

- The root surface is one process with an enumerable API; every operation is logged for the audit trail planned in MVP.
- Rust's safety guarantees apply to the most dangerous code in the project (see [[001-rust-for-control-plane-and-agent]]).
- Splitting agent and control plane into two processes later is a transport change, not a redesign.
