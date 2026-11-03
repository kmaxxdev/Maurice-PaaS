# ADR-010: Zero-downtime deploys and canary as weights on a workspace edge route

- **Status:** accepted
- **Date:** 2026-11-03
- **Issue:** #20

## Context

Stateless services must redeploy without dropping requests, and must roll back as easily. The plan also calls for a canary: a newer build serving a small, stable sample of real production users while the stable build keeps serving the rest. Both need two versions of a service running at once in the same network with the same data and secrets. Quadlet units are static, so a version has to be an instance, not an edit of a unit.

## Decision

A stateless service is rendered as a template unit `<service>@.container`; each deployed version is an instance (`web@v13`) with its own container name and DNS alias, started by the panel. The workspace edge (ADR-009) has one route per exposed service with upstream groups per instance set and weights on the groups.

- **Blue-green.** The new instance starts with weight 0, the panel waits for `Notify=healthy`, then sets the new group to 100 and the old to 0 in one admin API push, drains the old instance and stops it after its termination grace period. Rollback is the same push in reverse while the old instance is still up, and a restart of the old instance later.
- **Canary.** The new instance set is `canary`; its group gets a percentage. The route uses a cookie-based load-balancing policy with a weighted fallback, so a new visitor is assigned once and stays on the same set for the cookie lifetime. A header or cookie may force the canary for the team. Panel actions: start with image and percent, change percent, promote (canary becomes stable, old stable retires through the blue-green path), abort (100 % back, canary stopped).
- **Stateful services** restart in place with a warning and a backup first, as the plan says.

Blue-green is a canary that goes from 0 to 100 in one step after health, so there is one implementation.

## Alternatives considered

- **Two fixed slots (blue, green).** Simple, but container names stop telling which version runs and a canary needs a third slot anyway.
- **Pod per service with rolling replacement.** A pod shares one network namespace, so two containers cannot overlap on the same port.
- **DNS alias switch with `podman network connect/disconnect`.** Works for in-network consumers but gives no traffic split and relies on client DNS caches expiring. The alias stays for in-network references; the edge decides traffic.

## Consequences

- Deployments are rows that point at an instance; history and rollback fall out of that.
- Charts get deploy markers per instance set; logs and stats are shown per set.
- Applications must tolerate two versions running at once for the duration of a deploy, which is the usual contract of rolling deploys.
