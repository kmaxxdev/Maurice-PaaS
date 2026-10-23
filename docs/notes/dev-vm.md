# Development VM

The agent must be exercised against real system users, `systemd --user` and rootless Podman without touching the development machine's own users. A libvirt VM running Ubuntu 26.04 LTS stands in for the production host (ADR-008). A second, Fedora VM can be created for the SELinux path.

## One-time host setup

```
sudo apt install -y libvirt-daemon-system virtinst qemu-system-x86
sudo usermod -aG libvirt "$USER"    # then log out and in again
```

## Create and use

```
scripts/dev-vm.sh create    # ~5 minutes on first run: image download + package install
scripts/dev-vm.sh ssh
scripts/dev-vm.sh status
```

Defaults: name `maurice-dev-ubuntu`, Ubuntu 26.04 cloud image, 4 vCPU, 8 GB RAM, 40 GB disk, user `dev` with passwordless sudo, SSH key `~/.ssh/id_ed25519_kmaxx.pub`. Override with `MAURICE_VM_*` environment variables (see the script header). `MAURICE_VM_DISTRO=fedora scripts/dev-vm.sh create` adds `maurice-dev-fedora` next to it.

## Protection against accidental deletion

- The VM carries a libvirt title and description marking it as protected and naming this repository. `virsh list --all --title` shows it.
- Its disk lives in a dedicated storage pool `maurice` at `/var/lib/libvirt/images/maurice`, separate from the default pool other projects use.
- The script's `stop` only shuts down. Removing the VM requires `purge`, which asks you to type the VM name.

## What the VM has after first boot

Podman 5.7 with crun, netavark and aardvark-dns; `systemd-container` for `machinectl` and `systemd-run --machine`; AppArmor at the Ubuntu default (SELinux enforcing on the Fedora variant); `podman.socket` enabled for the `dev` user. Workspace users are created by the agent during tests, not by cloud-init.

## Verified on 2026-10-23

VM `maurice-dev-ubuntu`: Ubuntu 26.04.1, Podman 5.7.0, crun 1.21, AppArmor on. As user `dev`:

- `podman run --rm quay.io/podman/hello` works rootless; `podman.socket` runs under `systemd --user` with linger.
- Units rendered by `maurice render examples/shop.json`, copied into `~/.config/containers/systemd/`, pass `/usr/libexec/podman/quadlet -user -dryrun`.
- `systemctl --user start postgres.service` blocks ~45 s on the first pull and returns only once the health check passes (`Notify=healthy`). The container shows `(healthy)`, a sibling container resolves `postgres` through the workspace network, nothing listens on the host (`ss -ltn` has no 5432), the journal carries the container log, `stop` leaves no container behind.

Two generator bugs surfaced only here and were fixed: `DNS=` in a `.network` unit takes resolver IPs (DNS is on by default), and Quadlet already derives `Requires`/`After` from `Network=`.

## Agent selftest, 2026-10-23

`scripts/vm-run.sh host selftest` builds the binary, copies it into the VM and runs `maurice host selftest` as root. The run creates `ws-selftest` (system user, subuid range, linger, quadlet dir), stores a podman secret through the user's manager, writes the rendered units, starts postgres and blocks until healthy (~45 s on first pull), restarts and stops it, then deletes the user with purge and checks that no user, home or subuid entry remains. It passes.

Two host facts learned on the way, both now handled by the agent:

- `loginctl enable-linger` starts the user manager asynchronously. The agent starts `user@<uid>.service` explicitly and polls `systemctl --user -M <user>@ is-system-running` before the first call, otherwise `systemd-run -M` fails with "Transport endpoint is not connected".
- Ubuntu 26.04's AppArmor profile for pasta accepts signals only from unconfined peers, while podman carries the `podman` label. Stopping any rootless container then fails at `podman rm` with `rootless netns: kill network process: permission denied` and systemd marks the unit failed. `maurice host prepare` adds `signal (receive) peer=podman,` through the profile's local override and reloads it. This affects every Podman user on Ubuntu 26.04, not only Maurice.

## Panel in the VM, 2026-10-23

```
scripts/vm-run.sh host prepare      # AppArmor fix + system PostgreSQL (maurice-postgres.service)
scripts/vm-run.sh tier              # also installs marik / maurice-odysseus-stella symlinks
ssh dev@<ip> 'sudo systemd-run --unit maurice-dev --collect -p EnvironmentFile=/etc/maurice/maurice.env /usr/local/bin/maurice serve'
ssh dev@<ip> 'marik workspace create shop && marik plan apply <id> && marik workspace list'
```

Verified: create plan → apply creates `ws-shop` through the agent and stores the row; `marik` refuses
the destructive delete plan, `maurice` needs `--confirm`; after apply the host has no user, home or
subuid entry. Quadlet-generated units cannot be `systemctl enable`d; `[Install]` in the unit file
does that, so the installer only starts them.

## Services through the panel, 2026-10-23

Verified end to end on the VM: secret set → service upsert (plan shows the full unit as a diff) →
apply writes `postgres.container`, the volume and the network unit → deploy plan starts the unit and
blocks until healthy (~40 s) → `service logs` reads the user journal → a second service resolves
`{{postgres.url}}` and publishes `127.0.0.1:18080`, `curl` answers 200 → re-upsert without changes
plans nothing → `marik` refuses the destructive delete, `maurice --confirm` applies it → deleting the
workspace with a running service and secret-backed env leaves no user, subuid, home or container.

## Builds, 2026-10-23

`marik project set <ws> default --repo https://github.com/kmaxxdev/Maurice-PaaS.git --ref main`, then a
service with `dockerfilePath: examples/hello-app/Dockerfile`. The deploy plan shows the build step; apply
clones (shallow, fresh every time) and runs `podman build` as the workspace user through
`systemd-run -M`, tags the image `localhost/<ws>/<svc>:<sha12>`, rewrites the container unit with that
image and version, starts it. Measured ~14 s for the hello image; a second deploy rebuilds and restarts.
Private repositories: store the deploy key as a workspace secret and pass `--key-secret`; the agent
writes it to `~/.config/maurice/keys/<service>` with mode 0600 and uses it via `GIT_SSH_COMMAND`.

## Reboot and hardening, 2026-10-23

Rebooted the VM with a deployed postgres in one workspace: `user@<uid>.service` of every workspace
came back through linger and the deployed service returned on its own; the panel's PostgreSQL unit
too. A service that was defined but never deployed also started, which is wrong: units now get
`[Install] WantedBy=default.target` only on their first deploy. `host prepare` additionally writes
journald limits (4 GB, one month, per-service rate limit). Published host ports are checked across
all workspaces when planning an upsert, and service logs are read only since the workspace was
created, so a reused system uid does not leak a previous workspace's journal.
