# Development

## Layout

```
crates/maurice           the binary: CLI tiers, `serve`, `render`
crates/maurice-core      domain model and validator (no I/O)
crates/maurice-quadlet   model → Quadlet unit text
crates/maurice-agent     the privileged surface as a trait; DryRunAgent for now
crates/maurice-api       ConnectRPC services, generated from proto/ at build time
crates/maurice-db        PostgreSQL pool and embedded migrations
proto/maurice/v1         the API schema (source of truth for Rust and TypeScript)
packages/ui              @maurice/ui, the component kit (tokens.css + components)
packages/web             @maurice/web, the Solid SPA
scripts/dev-vm.sh        libvirt VM for testing the agent against real users
```

## Toolchain

Rust stable (see `rust-toolchain.toml`), `buf` and `protoc` (mise: `mise use -g buf protoc`), Node LTS with pnpm.

```
cargo build --workspace            # also compiles proto/ into maurice-api
cargo test --workspace
pnpm install
pnpm proto                         # regenerate packages/web/src/gen from proto/
pnpm typecheck
```

## Run

```
cargo run -- serve                 # API on 127.0.0.1:7700 with the dry-run agent
pnpm dev                           # SPA on 5173, proxies /maurice.v1.* to 7700
cargo run -- render examples/shop.json   # print the units a workspace would produce
```

Try the API without the UI:

```
curl -s -X POST http://127.0.0.1:7700/maurice.v1.Workspaces/List -H 'content-type: application/json' -d '{}'
```

## Tiers

`marik`, `maurice` and `maurice-odysseus-stella` are the same binary; the tier comes from the name it is invoked by (ADR-007). For local use: `ln -s target/debug/maurice target/debug/marik`.
