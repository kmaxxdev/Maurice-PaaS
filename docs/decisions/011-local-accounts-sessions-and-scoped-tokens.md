# ADR-011: Local accounts, sessions, scoped tokens, roles and an audit log

- **Status:** accepted
- **Date:** 2026-11-03
- **Issue:** #21

## Context

Once the panel is behind a domain, every call needs an identity. The schema already has tokens with scopes read / plan / apply bound to workspaces, and the CLI tiers (ADR-007) map onto those scopes. What is missing is how humans log in, how tokens are issued, and who may do what in which environment.

## Decision

- **Accounts.** Table `users` with login, argon2id password hash and a role: `admin`, `operator` or `viewer`. `maurice host prepare` creates the first admin and prints a one-time password.
- **Sessions.** Server-side session rows referenced by an HttpOnly, SameSite cookie with a 12-hour idle expiry; the SPA sends it automatically.
- **Tokens.** Created by a user, scoped to read / plan / apply and to a set of workspaces, flagged human or not. Apply on `prod` and `staging` requires a human principal. The CLI stores a token after `marik login`.
- **Roles.** viewer: read everywhere. operator: plan and apply in `dev`, plan in `staging` and `prod`. admin: everything, including users, tokens and environments.
- **Transport.** A Connect interceptor turns the cookie or `Authorization: Bearer` header into a principal; every handler checks scope and role; every apply writes an audit row with principal, plan, tier and outcome.

## Alternatives considered

- **OIDC only.** No passwords to handle, but reaching your own server then depends on an external provider and still needs a break-glass path. Planned later as a second login method on top of this one.
- **Forward-auth through Caddy (Authelia, Authentik).** The panel still needs roles and tokens for the CLI, MCP and bots, so no work is saved, and it adds an always-on service to the starter set.

## Consequences

- Small and offline-capable; one more table family (`users`, `sessions`, `audit`).
- Roles are evaluated per environment, which is why environments (ADR-012) land before auth.
- TOTP and OIDC can be added without touching tokens or roles.
