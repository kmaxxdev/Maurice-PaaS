-- Alpha schema. Rules from the data model are enforced here, not in code review.

create table workspaces (
    id            uuid primary key,
    name          text not null unique check (name ~ '^[a-z0-9][a-z0-9-]{0,30}$'),
    unix_user     text not null unique,
    uid           integer not null unique,
    subuid_start  bigint not null unique,
    subuid_len    integer not null default 65536,
    home          text not null,
    cpu_slice     text,
    -- Contiguous block of host ports; every host binding of the workspace lies inside it (ADR-012).
    port_block_start integer not null check (port_block_start between 1024 and 65535),
    port_block_size  integer not null check (port_block_size in (100, 500, 1000)),
    created_at    timestamptz not null default now(),
    exclude using gist (int4range(port_block_start, port_block_start + port_block_size) with &&)
);

-- An environment owns a Podman network, services and secrets inside a workspace (ADR-012).
create table environments (
    id            uuid primary key,
    workspace_id  uuid not null references workspaces(id) on delete cascade,
    name          text not null check (name in ('dev', 'staging', 'prod')),
    created_at    timestamptz not null default now(),
    unique (workspace_id, name)
);

create table projects (
    id            uuid primary key,
    workspace_id  uuid not null references workspaces(id) on delete cascade,
    name          text not null,
    git_repo      text,
    git_ref       text not null default 'main',
    git_key_secret_id uuid,
    unique (workspace_id, name)
);

create table services (
    id              uuid primary key,
    environment_id  uuid not null references environments(id) on delete cascade,
    project_id      uuid not null references projects(id) on delete cascade,
    name            text not null check (name ~ '^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$'),
    kind          text not null check (kind in ('stateless', 'stateful')),
    source_kind   text not null check (source_kind in ('image', 'build')),
    image_ref     text,
    dockerfile    text,
    build_context text,
    cpu           real,
    memory_bytes  bigint,
    isolated      boolean not null default false,
    health_cmd    text,
    created_at    timestamptz not null default now(),
    check ((source_kind = 'image' and image_ref is not null) or (source_kind = 'build' and dockerfile is not null))
);

-- A service name is its network alias, so it is unique per environment (one network each).
create unique index services_name_per_environment on services (environment_id, name);

create table ports (
    id              uuid primary key,
    service_id      uuid not null references services(id) on delete cascade,
    container_port  integer not null check (container_port between 1 and 65535),
    published       boolean not null default false,
    host_ip         text,
    host_port       integer check (host_port between 1 and 65535),
    unique (service_id, container_port),
    check ((published and host_ip is not null and host_port is not null) or (not published and host_ip is null and host_port is null))
);

-- Routes on the workspace edge (ADR-009): a block port that proxies to a container port.
create table routes (
    id              uuid primary key,
    workspace_id    uuid not null references workspaces(id) on delete cascade,
    environment_id  uuid not null references environments(id) on delete cascade,
    service_id      uuid not null references services(id) on delete cascade,
    container_port  integer not null check (container_port between 1 and 65535),
    block_port      integer not null check (block_port between 1024 and 65535),
    visibility      text not null check (visibility in ('public', 'internal')),
    created_at      timestamptz not null default now(),
    unique (workspace_id, block_port),
    -- A container port may be exposed once publicly and once internally.
    unique (service_id, container_port, visibility)
);

-- Domains the host edge attaches to public routes.
create table domains (
    id          uuid primary key,
    route_id    uuid not null references routes(id) on delete cascade,
    name        text not null unique,
    public_ip   text,
    tls_mode    text not null check (tls_mode in ('auto', 'internal', 'off')),
    created_at  timestamptz not null default now()
);

create table env_vars (
    id          uuid primary key,
    service_id  uuid not null references services(id) on delete cascade,
    key         text not null check (key ~ '^[A-Za-z_][A-Za-z0-9_]*$'),
    value_kind  text not null check (value_kind in ('literal', 'ref', 'secret')),
    value       text,
    secret_id   uuid,
    unique (service_id, key),
    check ((value_kind = 'secret' and secret_id is not null and value is null) or (value_kind <> 'secret' and value is not null))
);

-- The only table that ever holds a secret value, and only encrypted.
-- environment_id null = shared by the whole workspace (deploy keys); set = that environment only.
create table secrets (
    id              uuid primary key,
    workspace_id    uuid not null references workspaces(id) on delete cascade,
    environment_id  uuid references environments(id) on delete cascade,
    service_id      uuid references services(id) on delete cascade,
    name            text not null,
    encrypted_value bytea not null,
    version         integer not null default 1,
    created_at      timestamptz not null default now(),
    unique nulls not distinct (workspace_id, environment_id, name, version)
);

alter table env_vars add constraint env_vars_secret_fk foreign key (secret_id) references secrets(id) on delete cascade;
alter table projects add constraint projects_git_key_fk foreign key (git_key_secret_id) references secrets(id) on delete set null;

create table plans (
    id            uuid primary key,
    -- Not a foreign key: a plan that creates a workspace exists before the workspace does
    -- (workspace_id is the nil uuid until apply), and plans outlive deleted workspaces as history.
    workspace_id  uuid not null,
    changes       jsonb not null,
    findings      jsonb not null,
    destructive   boolean not null default false,
    created_by    text not null,
    created_at    timestamptz not null default now(),
    expires_at    timestamptz not null,
    applied_at    timestamptz,
    discarded_at  timestamptz
);

create table deployments (
    id            uuid primary key,
    service_id    uuid not null references services(id) on delete cascade,
    plan_id       uuid not null references plans(id),
    image_digest  text not null,
    git_sha       text,
    status        text not null check (status in ('pending', 'building', 'starting', 'healthy', 'failed', 'rolled_back')),
    started_at    timestamptz not null default now(),
    finished_at   timestamptz
);

-- Who may call the panel (ADR-011). Passwords are argon2id PHC strings.
create table users (
    id            uuid primary key,
    login         text not null unique check (login ~ '^[a-z0-9][a-z0-9._-]{0,62}$'),
    password_hash text not null,
    role          text not null check (role in ('admin', 'operator', 'viewer')),
    disabled      boolean not null default false,
    created_at    timestamptz not null default now()
);

-- Browser sessions: the cookie carries a random id, the table its sha256.
create table sessions (
    id            uuid primary key,
    user_id       uuid not null references users(id) on delete cascade,
    hash          bytea not null unique,
    created_at    timestamptz not null default now(),
    expires_at    timestamptz not null,
    last_seen_at  timestamptz not null default now()
);

-- API tokens for the CLI, MCP and bots: the secret is shown once, the table keeps its sha256.
create table tokens (
    id            uuid primary key,
    user_id       uuid not null references users(id) on delete cascade,
    name          text not null,
    hash          bytea not null unique,
    scopes        text[] not null check (scopes <@ array['read', 'plan', 'apply']),
    workspace_ids uuid[] not null default '{}',
    -- Apply on prod and staging needs a human behind the token.
    human         boolean not null default true,
    created_at    timestamptz not null default now(),
    expires_at    timestamptz,
    last_used_at  timestamptz,
    revoked_at    timestamptz
);

-- One row per state-changing call: who did what, through which tier, and how it ended.
create table audit (
    id            uuid primary key,
    at            timestamptz not null default now(),
    user_id       uuid references users(id) on delete set null,
    principal     text not null,
    action        text not null,
    target        text not null,
    tier          text not null default '',
    outcome       text not null check (outcome in ('ok', 'denied', 'failed')),
    detail        text not null default ''
);
create index audit_at on audit (at desc);
