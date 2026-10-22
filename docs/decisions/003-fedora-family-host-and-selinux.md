# ADR-003: Fedora-family host, SELinux-aware generation

- **Status:** accepted
- **Date:** 2026-10-22
- **Issue:** #4

## Context

Quadlet in a usable form starts with Podman 5. Podman is developed by Red Hat and packaged for Fedora first; Debian follows with a delay and freezes for two years; Ubuntu follows Debian and has no official upstream Podman repository. Ubuntu 24.04 LTS ships Podman 4.9, which is too old. The current production server runs Ubuntu.

## Decision

- **Development and Alpha target: Fedora Server** with the current Podman 5.x, crun, netavark and aardvark-dns.
- **Second supported target: Debian 13** (Podman 5.4). Verified in CI once there is CI.
- **Production host** will be reinstalled onto the Fedora family at the Coolify migration (MVP). The final pick between Fedora Server (freshest, yearly release upgrade) and AlmaLinux 10 (ten-year support, more conservative Podman) is made before MVP.
- **Ubuntu is not a target for Alpha.** Ubuntu 26.04 LTS may qualify later; to be checked.
- **SELinux is assumed enforcing.** The Quadlet generator always emits `:Z` (or `:z` for shared) on bind mounts and named volumes and sets labels where needed. This is a hard rule, not an option.

## Alternatives considered

- **Stay on Ubuntu and build Podman from source.** Permanent manual work; rejected.
- **Fedora CoreOS** (immutable, auto-updating, Podman-first). Attractive for a pure PaaS host but awkward for bare-metal nodes that need custom kernel tuning and packages. Keep as a future option.

## Consequences

- Spike #4 sets up a Fedora Server VM reachable over SSH; a Debian 13 VM follows when CI exists.
- Blockchain nodes and other heavy services run the same on Fedora as on Ubuntu; their requirements are kernel sysctls and hardware, not the distribution. The one thing to watch is SELinux labels on data directories.
- The small-server case (1 vCPU, 1 GB VPS) stays viable: the starter set is the panel, PostgreSQL and Caddy; registry, Dragonfly, VictoriaMetrics and the rest are catalogue services the user adds.
