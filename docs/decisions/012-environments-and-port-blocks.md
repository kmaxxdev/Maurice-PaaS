# ADR-012: Environments inside a workspace and a port block per workspace

- **Status:** accepted
- **Date:** 2026-11-03
- **Issue:** #34, #35

## Context

Alpha gave a workspace a single `env` label, so a project with development and production needed two workspaces and two system users. Real projects also need a staging environment that runs near-production code against real providers, and a canary that is a variant of production rather than a place of its own. Host ports were chosen by hand per service, which made conflicts between workspaces something to check rather than something impossible.

## Decision

**Environments.** A workspace owns a set of environments from `dev`, `staging`, `prod`, chosen at creation and extensible later. Each environment has its own Podman network, services, env vars, secrets, deployments and workspace edge (ADR-009); unit names are prefixed with the environment. The workspace keeps one system user, one home and one port block. Policy follows the environment: `dev` auto-applies webhooks, `staging` and `prod` accept apply only from a human principal (ADR-011). In-environment references like `{{cache.url}}` resolve inside the same environment. Canary is an instance set inside `prod` (ADR-010), not an environment. Promotion copies a deployment's image digest from one environment to the next as a plan.

**Port block.** Each workspace receives a contiguous block of host ports at creation, taken from a host range configured in `maurice.env` (default 20000–39999), with a block size of 100, 500 (default) or 1000. Everything that binds a host port for that workspace lives inside the block: workspace edge listeners, internal link routes, direct TCP publishes and both instances of a blue-green pair. A port outside the block is refused. The block is released when the workspace is deleted.

## Alternatives considered

- **Environment per workspace (Alpha).** Clear isolation, but doubles users, blocks and edges per project and makes promotion a cross-workspace copy.
- **Environments as a label on services.** No network isolation; a staging service could reach the production database by name.
- **Global port allocator with a conflict check.** Works, but ports read as random numbers and the check is a rule rather than a structure.

## Consequences

- Migration 0001 gains an `environments` table and moves service-level tables under it; being pre-1.0, the migration is edited in place and dev databases are reset.
- Cross-workspace conflicts become impossible by construction, and a workspace's ports read as a range on its page.
- Blocks cap how many host ports a workspace may use; the size is chosen with that in mind and can be raised by reallocating.
