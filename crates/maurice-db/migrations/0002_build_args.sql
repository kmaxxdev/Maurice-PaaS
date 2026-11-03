-- Build arguments for services built from a Dockerfile (`--build-arg KEY=value`).
alter table services add column build_args jsonb not null default '{}'::jsonb;
