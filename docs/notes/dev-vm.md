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
