# ADR-007: Three CLI entry points as safety tiers

- **Status:** accepted
- **Date:** 2026-10-22
- **Issue:** —

## Context

The panel ships one binary that is also the CLI. Operators run it by hand, in scripts and from agents. Destructive operations (delete workspace, purge volumes, drop database users) must be hard to run by accident, while everyday operations must stay short to type. The token model already defines scopes read / plan / apply.

## Decision

One binary, three names (symlinks), each mapping to a safety tier that reuses the token scopes:

| Entry point | Allowed | Behaviour |
|---|---|---|
| `marik` | read, plan, and reversible apply: status, logs, stats, deploy with rollback, restart, publish/unpublish port | Refuses destructive operations with a hint to use `maurice` |
| `maurice` | everything | Destructive operations print the plan and require interactive confirmation |
| `maurice-odysseus-stella` | everything, non-interactive | Skips confirmation for scripted recovery; additionally requires `--yes-i-mean-it` or `MAURICE_FORCE=1`; every call is written to the audit log flagged as forced |

The server unit is `maurice.service`. "Maurice Odysseus Stella" is the project's full name used in banners and documentation.

An operation is classified as destructive when it deletes data or users, or cannot be undone from the panel: workspace delete, volume purge, secret delete, DB user drop, backup delete, user removal from the panel.

## Alternatives considered

- **Single name with `--force`.** Standard, but easy to add to a shell alias; the tiered names make the intent visible in shell history and in audit logs.
- **Separate binaries.** Same effect with more packaging work and three artefacts to version.

## Consequences

- The CLI dispatch reads `argv[0]` to pick the tier; the tier is passed down as a scope so the same checks apply to API tokens and to the CLI.
- Shell aliasing of the long name does not bypass the extra flag or environment variable.
- Agents and MCP clients get `marik`-level scopes by default (see the token model in the Architecture page).
