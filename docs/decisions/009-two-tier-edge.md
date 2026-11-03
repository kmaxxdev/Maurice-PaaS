# ADR-009: Two-tier edge: host Caddy and a workspace edge per environment

- **Status:** accepted
- **Date:** 2026-11-03
- **Issue:** #19

## Context

Public traffic enters on ports 80 and 443, which only a root process can hold on the shared public IP. Containers run rootless inside per-workspace Podman networks that a root process cannot join, so something must bridge the two. The same bridge has to switch versions during a blue-green deploy, split traffic for a canary, and let one workspace reach another workspace's service without exposing it to the internet. Alpha published container ports straight to the host, which works for a demo but means every version switch is a host-wide change.

## Decision

Two tiers of Caddy, both driven by the panel through the admin API and both rebuilt from the panel's tables on start.

**Host edge.** One rootful Caddy as a system Quadlet unit with `Network=host`. It terminates TLS (HTTP-01 per domain, DNS-01 wildcard per zone with a provider token kept as a panel secret) and maps each domain to `127.0.0.1:<port>` where the port is the public listener of a workspace edge. Its config changes only when domains change.

**Workspace edge.** One rootless Caddy per environment, rendered as a Quadlet unit alongside the environment's network. It resolves containers by their DNS aliases, holds the routes table of that environment and publishes routes on ports from the workspace's port block (ADR-012). A route is either public, in which case the host edge attaches a domain to it, or internal, in which case other workspaces may reference it through a link (`{{shop.web.url}}`). Blue-green and canary only change upstream weights on a route here (ADR-010).

"Expose" in the panel means a route on the workspace edge. Containers publish no host ports of their own; the Alpha-style direct `PublishPort` remains as an escape hatch for raw TCP services.

## Alternatives considered

- **One host Caddy with per-instance loopback ports (ZaneOps shape).** Fewer moving parts, but every version switch of every workspace is a change to the public tier, and the host edge has to know every container. Kept as the fallback if the workspace edge proves too heavy.
- **Caddyfile and `caddy reload` instead of the admin API.** Human-readable, but slower, not transactional across many sites, and a second source of truth. The panel may export a Caddyfile for inspection, but drives Caddy through the API.
- **Rootless Caddy per workspace as the only tier.** Needs a public IP per workspace because 80/443 can be held once per address, and scatters certificates. It returns here as the inner tier.
- **Forward-auth or a service mesh in front of containers.** Out of scope for a single server.

## Implementation notes (2026-11-03)

- The workspace edge is one container per **workspace**, not per environment: it joins every
  environment network and publishes the workspace's port block once, which is what lets routes
  change without restarting it. Upstreams are container names (which carry the version), so
  environments never collide on aliases.
- Ports a service publishes directly are carved out of the edge's ranges; the edge unit is
  rewritten and restarted when that set changes, routes only reload.
- Both edges are driven through a config file and `caddy reload` over a unix admin socket
  (`podman exec`), so no admin API is reachable over the network.
- Internal routes bind to a dedicated host address (`MAURICE_INTERNAL_ADDR`, default 10.200.0.1
  on a dummy interface) because rootless containers cannot reach the host's loopback.

## Consequences

- The public tier is small and stable; blast radius of a deploy is one workspace.
- A workspace is self-describing: its edge config is the map of what runs inside it, which is what "files on disk are the truth" asked for.
- One more container per environment (Caddy, about 40 MB RSS).
- To verify in the VM before implementation: rootless Caddy binding block ports through pasta, and which host address rootless containers reach for loopback-bound internal routes. If plain loopback is not reachable, internal routes bind to a dedicated internal host address.
