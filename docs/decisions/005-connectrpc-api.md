# ADR-005: ConnectRPC for the panel API

- **Status:** accepted
- **Date:** 2026-10-22
- **Issue:** —

## Context

The UI, CLI, MCP server, Telegram bot and external clients all talk to the panel. Candidates: REST with OpenAPI, gRPC, ConnectRPC, GraphQL, tRPC.

## Decision

ConnectRPC with a Protobuf schema as the single source of truth for the API. Servers and clients are generated from `proto/`: Rust for the panel, TypeScript for the Solid UI and the MCP server. The protocol works from the browser over plain HTTP with JSON or binary encoding and stays compatible with gRPC clients.

## Alternatives considered

- **REST + OpenAPI.** Universally understood and curl-friendly, but the contract drifts from the code and types are weaker. Kept as a documented option if ConnectRPC tooling for Rust proves immature.
- **Plain gRPC.** Same schema benefits, but needs a proxy for browsers and is harder to debug.
- **GraphQL.** Suited to flexible reads, unnatural for imperative operations like deploy or publish-port.
- **tRPC.** Requires a TypeScript server.

## Consequences

- One `.proto` file set describes the data model and every operation; the plan/apply boundary is visible in the schema (plan requests return a plan, apply takes a plan id).
- Token scopes (read / plan / apply) map to RPC methods.
- A Rust ConnectRPC implementation is chosen during the first API crate; if none is production-grade, the fallback is tonic (gRPC) behind a Connect-compatible gateway, with REST as a last resort.
