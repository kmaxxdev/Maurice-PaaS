<div align="center">

# Maurice PaaS

**A self-hosted deployment panel for one powerful server, built on rootless Podman.**

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Status](https://img.shields.io/badge/status-pre--alpha-orange.svg)](https://github.com/kmaxxdev/Maurice-PaaS/wiki/Roadmap)
[![Board](https://img.shields.io/badge/tasks-project%20board-8A2BE2.svg)](https://github.com/users/kmaxxdev/projects/1)
[![Wiki](https://img.shields.io/badge/docs-wiki-informational.svg)](https://github.com/kmaxxdev/Maurice-PaaS/wiki)

</div>

Maurice (friends call it *Marik*) gives you the convenience of Coolify on a single
bare-metal server where blockchain nodes, ClickHouse, databases and application
services live side by side. It is not Docker with a different runtime underneath.
It is built around what Podman does well: rootless isolation by Unix user,
Quadlet and systemd as the lifecycle engine, auto-update with rollback, native
secrets.

> **Status:** pre-alpha. The plan is written, the code is not. Follow progress on
> the [task board](https://github.com/users/kmaxxdev/projects/1).

## Why another panel

| The pain | What Maurice does about it |
|---|---|
| Ports exposed by default, closed by hand | Nothing is published until you tick a box. A port lives inside the workspace network until then. |
| `docker.sock` access equals root on a host full of keys | Every workspace is its own system user running rootless Podman. One compromised project cannot reach the others or the host. |
| Hand-copied configs, typos in env, port clashes | The panel generates Quadlet units from a validated model. Env references like `{{postgres.url}}` are resolved for you. |
| Admin DB credentials in every service | Database roles (admin / app / readonly) with rotation that restarts the dependents. |
| Logs filling the disk | journald with limits, searchable in the UI. |
| Agents that can wreck prod | Plan / apply. Anything an agent or external client proposes is a plan a human confirms. In prod, always. |
| Development tied to a laptop | Dev workspaces on the server: SSH, tmux, Neovim, Claude Code, code-server. The laptop can reboot. |

## How it works

```
User / Claude Code / Telegram
        │
        ▼
  UI + API + MCP            tokens scoped read / plan / apply
        │
        ▼
  Control plane             generates Quadlet units, change plans, Caddy config
        │
        ▼
  Agent ── machinectl ──▶  ws-<project>   rootless Podman + systemd --user
        │                    ├─ web@.service        stateless, blue-green
        │                    ├─ postgres.service    stateful, restart + backups
        │                    └─ <project>.network   DNS by service name
        ▼
  Caddy admin API           domain ──▶ 127.0.0.1:<port>, TLS, graceful switch
```

Files on disk stay the source of truth: every unit the panel writes can be read,
diffed and applied with plain `systemctl`. The panel is a convenience layer, not
a runtime dependency.

## Roadmap

| Stage | Purpose | Status |
|---|---|---|
| **Alpha** | Panel → agent → Quadlet → rootless Podman works on a real server. Workspaces, deploy from image or Dockerfile, logs, networks, secrets, validator. | in planning |
| **MVP** | Fit for production. Caddy with TLS, blue-green deploys, registry, webhooks, service catalogue, Telegram alerts, S3 backups, telemetry, auth. | — |
| **Beta** | Full feature set. MCP server, built-in agent, remote dev workspaces, Cloudflare tunnels, log triage, Plane tracker. | — |
| **Release** | Stabilisation. Docs, upgrade procedure, Podman compatibility matrix, regression tests. | — |

Scope, done criteria and risks for every stage are in the
[Roadmap](https://github.com/kmaxxdev/Maurice-PaaS/wiki/Roadmap).

## Stack

Rust for the control plane and agent, one static binary · Podman 5 with crun,
netavark and aardvark-dns · systemd + Quadlet · PostgreSQL · ConnectRPC API ·
SolidJS SPA embedded in the binary · Caddy · VictoriaMetrics and VictoriaLogs ·
restic + rclone · Fedora Server host, Debian 13 as second target. Every choice
has a record in [`docs/decisions/`](docs/decisions/) and the reasoning in the
[Technology Stack](https://github.com/kmaxxdev/Maurice-PaaS/wiki/Technology-Stack).

## Learn more

- [Wiki](https://github.com/kmaxxdev/Maurice-PaaS/wiki): the full plan, architecture, data model, prior art, telemetry, remote development.
- [Task board](https://github.com/users/kmaxxdev/projects/1) and [issues](https://github.com/kmaxxdev/Maurice-PaaS/issues): what is being worked on right now.
- [`docs/`](docs/): architecture decision records and working notes, versioned with the code. Opens as an Obsidian vault.
- [Contributing](https://github.com/kmaxxdev/Maurice-PaaS/wiki/Contributing): language, commit format, branch workflow.

## License

[Apache 2.0](LICENSE). Maurice stands on the shoulders of
[Coolify](https://github.com/coollabsio/coolify),
[Rookery](https://github.com/rookerylabs/rookery), Podman, Caddy and systemd.
