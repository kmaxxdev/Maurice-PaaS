# Development VM

The agent must be exercised against real system users, `systemd --user`, rootless Podman and SELinux. The development machine runs Ubuntu, so a Fedora Server VM under libvirt stands in for the target host (see ADR-003).

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

Defaults: name `maurice-dev-fedora`, Fedora 42 cloud image, 4 vCPU, 8 GB RAM, 40 GB disk, user `dev` with passwordless sudo, SSH key `~/.ssh/id_ed25519_kmaxx.pub`. Override with `MAURICE_VM_*` environment variables (see the script header).

## Protection against accidental deletion

- The VM carries a libvirt title and description marking it as protected and naming this repository. `virsh list --all --title` shows it.
- Its disk lives in a dedicated storage pool `maurice` at `/var/lib/libvirt/images/maurice`, separate from the default pool other projects use.
- The script's `stop` only shuts down. Removing the VM requires `purge`, which asks you to type the VM name.

## What the VM has after first boot

Podman 5 with crun, netavark and aardvark-dns; `systemd-container` for `machinectl` and `systemd-run --machine`; SELinux enforcing (Fedora default); `podman.socket` enabled for the `dev` user. Workspace users are created by the agent during tests, not by cloud-init.
