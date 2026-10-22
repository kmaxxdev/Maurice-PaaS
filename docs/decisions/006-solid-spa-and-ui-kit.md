# ADR-006: Solid SPA embedded in the binary, own UI kit

- **Status:** accepted
- **Date:** 2026-10-22
- **Issue:** #13

## Context

The panel needs a web UI. The plan mentioned SolidStart or React. SolidStart is a server-rendering framework that requires a Node runtime on the host.

## Decision

- **SolidJS single-page application built with Vite**, no server rendering. The build output is static and embedded into the panel binary, so deployment stays one file.
- **Own component library** developed as `packages/ui` inside the monorepo, with design tokens as CSS variables and a dark theme from the start. It is extracted into its own repository and published once its API settles.
- **Accessibility primitives from Kobalte** (headless, unstyled Solid components) for menus, dialogs, tabs and similar; our styling on top.
- API access through the TypeScript client generated from the ConnectRPC schema (see [[005-connectrpc-api]]).

## Alternatives considered

- **SolidStart.** Adds a Node runtime and SSR the panel does not need.
- **React.** Larger ecosystem, but the team's current projects and preference are Solid.
- **Third-party UI kit.** Faster start, but the long-term goal is a kit the community can build on.

## Consequences

- Alpha UI is functional, not pretty; the kit grows with the screens that are actually needed.
- Keeping the kit in the monorepo until extraction avoids synchronising two repositories on every component change.
