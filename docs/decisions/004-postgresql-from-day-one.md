# ADR-004: PostgreSQL from day one

- **Status:** accepted
- **Date:** 2026-10-22
- **Issue:** —

## Context

The panel stores metadata: workspaces, services, deployments, secrets, tokens. Volume is thousands of rows, load is a few queries per second. SQLite would be enough for Alpha and has zero operational cost. However panel user accounts, roles and the audit log are planned right after the data layer exists, and a migration from SQLite to PostgreSQL in the middle of that work costs more than running PostgreSQL from the start.

## Decision

PostgreSQL is the only supported database for panel state, from the first commit. Access through `sqlx` with compile-time checked queries and versioned migrations. The panel ships PostgreSQL as part of its starter set (panel + PostgreSQL + Caddy) and manages it as an ordinary stateful service with backups.

## Alternatives considered

- **SQLite for Alpha, PostgreSQL at MVP.** Simplest install, but two code paths or a migration right when auth and audit land.
- **Distributed SQL (CockroachDB, TiDB, Yugabyte).** Built for clusters; pure overhead on one server.
- **Embedded KV stores.** No SQL for audit queries and reporting.

## Consequences

- The installer must bring up PostgreSQL before the panel; the panel must not depend on the services it manages for anything else.
- Panel state backups are a PostgreSQL dump, scheduled like any other backup job.
- Performance of managed workloads is unaffected: containers live in systemd and cgroups, the panel is not on their data path.
