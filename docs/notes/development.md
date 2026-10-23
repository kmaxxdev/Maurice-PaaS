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

The panel needs PostgreSQL. On a prepared host (`sudo maurice host prepare` installs it as a
system Quadlet unit and writes `/etc/maurice/maurice.env`) just run `sudo maurice serve`.
Locally, point it at any PostgreSQL and use the dry-run agent:

```
podman run -d --name maurice-pg -e POSTGRES_PASSWORD=dev -e POSTGRES_USER=maurice -e POSTGRES_DB=maurice -p 127.0.0.1:5432:5432 docker.io/library/postgres:16
cargo run -- serve --dry-run --database-url postgres://maurice:dev@127.0.0.1:5432/maurice
pnpm dev                                  # SPA on 5173, proxies /maurice.v1.* to 7700
cargo run -- render examples/shop.json    # print the units a workspace would produce
```

Client commands talk to the panel (`--server`, default `http://127.0.0.1:7700`):

```
marik workspace create shop               # returns a plan
marik plan apply <plan-id>                # creates the user through the agent
marik workspace list
maurice workspace delete <id> --purge     # destructive plan
maurice plan apply <plan-id> --confirm    # marik refuses this one
```

In the VM: `scripts/vm-run.sh host prepare`, then start the panel as a transient unit and use the
same commands over SSH (see `docs/notes/dev-vm.md`).

Try the API without the UI:

```
curl -s -X POST http://127.0.0.1:7700/maurice.v1.Workspaces/List -H 'content-type: application/json' -d '{}'
```

## Tiers

`marik`, `maurice` and `maurice-odysseus-stella` are the same binary; the tier comes from the name it is invoked by (ADR-007). For local use: `ln -s target/debug/maurice target/debug/marik`.
