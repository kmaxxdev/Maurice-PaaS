#!/usr/bin/env bash
# Create (or manage) the local VM used to develop and test the Maurice agent
# against real system users and rootless Podman. Ubuntu 26.04 LTS by default
# (the primary host, ADR-008); Fedora optional for SELinux compatibility checks.
#
# The VM is deliberately hard to delete by accident:
#   - a distinctive name and a libvirt title/description marking it as protected
#   - its own storage pool under /var/lib/libvirt/images/maurice
#   - `destroy` here only stops it; removal requires the explicit `purge` verb
#
# Requirements on the host: libvirt-daemon-system, virtinst, qemu-system-x86,
# a user in the libvirt group, and an SSH public key (default: ~/.ssh/id_ed25519_kmaxx.pub).
#
# Usage:
#   scripts/dev-vm.sh create     # download image, create pool and VM, wait for SSH
#   MAURICE_VM_DISTRO=fedora scripts/dev-vm.sh create   # second VM: maurice-dev-fedora
#   scripts/dev-vm.sh ssh        # open a shell in the VM
#   scripts/dev-vm.sh ip         # print the VM address
#   scripts/dev-vm.sh stop       # graceful shutdown
#   scripts/dev-vm.sh start
#   scripts/dev-vm.sh purge      # remove VM and disk (asks for confirmation)
set -euo pipefail

DISTRO="${MAURICE_VM_DISTRO:-ubuntu}"            # ubuntu | fedora
UBUNTU_RELEASE="${MAURICE_UBUNTU_RELEASE:-26.04}"
FEDORA_RELEASE="${MAURICE_FEDORA_RELEASE:-42}"
VM_NAME="${MAURICE_VM_NAME:-maurice-dev-${DISTRO}}"
VCPUS="${MAURICE_VM_VCPUS:-4}"
RAM_MB="${MAURICE_VM_RAM_MB:-8192}"
DISK_GB="${MAURICE_VM_DISK_GB:-40}"
SSH_PUBKEY="${MAURICE_VM_SSH_PUBKEY:-$HOME/.ssh/id_ed25519_kmaxx.pub}"
SSH_USER="dev"
SSH_OPTS=(-i "${SSH_PUBKEY%.pub}" -o IdentitiesOnly=yes)
POOL_NAME="maurice"
POOL_DIR="/var/lib/libvirt/images/maurice"
CONNECT="qemu:///system"
WORK_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/maurice-dev-vm"

virsh() { command virsh -c "$CONNECT" "$@"; }

die() { echo "error: $*" >&2; exit 1; }

need() { command -v "$1" >/dev/null 2>&1 || die "missing command: $1 (install libvirt-daemon-system virtinst qemu-system-x86)"; }

resolve_image_url() {
  case "$DISTRO" in
    ubuntu)
      echo "https://cloud-images.ubuntu.com/releases/${UBUNTU_RELEASE}/release/ubuntu-${UBUNTU_RELEASE}-server-cloudimg-amd64.img" ;;
    fedora)
      # The exact qcow2 file name carries a build number, so read it from the directory listing.
      local base="https://download.fedoraproject.org/pub/fedora/linux/releases/${FEDORA_RELEASE}/Cloud/x86_64/images/"
      local name
      name=$(curl -fsSL "$base" | grep -oE 'Fedora-Cloud-Base-Generic[^"]*x86_64\.qcow2' | head -1)
      [ -n "$name" ] || die "could not find a Fedora ${FEDORA_RELEASE} cloud image at $base"
      echo "${base}${name}" ;;
    *) die "unknown MAURICE_VM_DISTRO: $DISTRO (ubuntu|fedora)" ;;
  esac
}

osinfo_name() {
  # osinfo-db on the host may lag behind the release we install; it only sets
  # virtual hardware defaults, so fall back to the newest entry it does know.
  local want dir prefix
  case "$DISTRO" in
    ubuntu) want="ubuntu${UBUNTU_RELEASE}"; dir=/usr/share/osinfo/os/ubuntu.com; prefix=ubuntu- ;;
    fedora) want="fedora${FEDORA_RELEASE}"; dir=/usr/share/osinfo/os/fedoraproject.org; prefix=fedora- ;;
  esac
  if [ -f "$dir/${prefix}${want#"$DISTRO"}.xml" ]; then echo "$want"; return; fi
  local newest
  newest=$(ls "$dir"/${prefix}[0-9]*.xml 2>/dev/null | sed -E "s|.*/${prefix}([0-9.]+)\.xml|\1|" | sort -V | tail -1)
  [ -n "$newest" ] && echo "${DISTRO}${newest}" || echo "generic"
}

distro_packages() {
  case "$DISTRO" in
    ubuntu) printf '  - %s\n' podman crun netavark aardvark-dns systemd-container uidmap slirp4netns passt apparmor-utils git rsync ;;
    fedora) printf '  - %s\n' podman crun netavark aardvark-dns systemd-container policycoreutils-python-utils git rsync ;;
  esac
}

write_cloud_init() {
  local key; key=$(cat "$SSH_PUBKEY")
  mkdir -p "$WORK_DIR"
  cat > "$WORK_DIR/user-data" <<CLOUD
#cloud-config
hostname: ${VM_NAME}
users:
  - name: ${SSH_USER}
    groups: [wheel]
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    ssh_authorized_keys:
      - ${key}
package_update: true
packages:
$(distro_packages)
runcmd:
  # Keep the dev user's systemd --user alive without a login so its podman.socket can run.
  - [ loginctl, enable-linger, ${SSH_USER} ]
  - [ sh, -c, "sleep 2; systemctl --user -M ${SSH_USER}@ enable --now podman.socket || true" ]
  - [ sh, -c, "echo 'Maurice dev VM ready' > /etc/motd" ]
CLOUD
  cat > "$WORK_DIR/meta-data" <<META
instance-id: ${VM_NAME}
local-hostname: ${VM_NAME}
META
}

ensure_pool() {
  if ! virsh pool-info "$POOL_NAME" >/dev/null 2>&1; then
    sudo mkdir -p "$POOL_DIR"
    virsh pool-define-as "$POOL_NAME" dir --target "$POOL_DIR" >/dev/null
    virsh pool-build "$POOL_NAME" >/dev/null
    virsh pool-start "$POOL_NAME" >/dev/null
    virsh pool-autostart "$POOL_NAME" >/dev/null
  fi
}

cmd_create() {
  need virsh; need virt-install; need qemu-img; need curl
  [ -f "$SSH_PUBKEY" ] || die "SSH public key not found: $SSH_PUBKEY"
  if virsh dominfo "$VM_NAME" >/dev/null 2>&1; then
    die "VM '$VM_NAME' already exists; use 'start' or 'purge'"
  fi
  ensure_pool
  mkdir -p "$WORK_DIR"
  local url base_img
  url=$(resolve_image_url)
  base_img="$WORK_DIR/$(basename "$url")"
  if [ ! -f "$base_img" ]; then
    echo "downloading $url"
    curl -fL --progress-bar -o "$base_img.part" "$url" && mv "$base_img.part" "$base_img"
  fi
  local disk="$POOL_DIR/${VM_NAME}.qcow2"
  echo "creating disk $disk (${DISK_GB}G, backed by the downloaded image)"
  sudo qemu-img create -q -f qcow2 -F qcow2 -b "$base_img" "$disk" "${DISK_GB}G"
  sudo cp "$base_img" "$POOL_DIR/$(basename "$base_img")"   # keep the backing file inside the pool
  sudo qemu-img rebase -q -u -F qcow2 -b "$POOL_DIR/$(basename "$base_img")" "$disk"
  virsh pool-refresh "$POOL_NAME" >/dev/null
  write_cloud_init
  echo "defining VM $VM_NAME"
  virt-install --connect "$CONNECT" \
    --name "$VM_NAME" \
    --metadata title="Maurice PaaS dev VM (${DISTRO})" \
    --metadata description="PROTECTED: development VM for the Maurice PaaS project (${DISTRO}, Podman 5). Do not delete from other projects. Managed by scripts/dev-vm.sh in the Maurice-PaaS repository." \
    --osinfo "name=$(osinfo_name),detect=on,require=off" \
    --vcpus "$VCPUS" --memory "$RAM_MB" \
    --disk "path=$disk,format=qcow2,bus=virtio" \
    --cloud-init "user-data=$WORK_DIR/user-data,meta-data=$WORK_DIR/meta-data" \
    --network network=default,model=virtio \
    --graphics none --console pty,target_type=serial \
    --import --noautoconsole --autostart
  echo "waiting for an IP address"
  local ip=""
  for _ in $(seq 1 60); do
    ip=$(cmd_ip 2>/dev/null || true)
    [ -n "$ip" ] && break
    sleep 2
  done
  [ -n "$ip" ] || die "VM has no IP yet; try '$0 ip' in a minute"
  echo "waiting for cloud-init to finish (packages install on first boot)"
  for _ in $(seq 1 90); do
    if ssh "${SSH_OPTS[@]}" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=3 -o BatchMode=yes "$SSH_USER@$ip" 'cloud-init status --wait >/dev/null 2>&1 && podman --version' 2>/dev/null; then
      echo "ready: ssh $SSH_USER@$ip   (or: $0 ssh)"
      return 0
    fi
    sleep 5
  done
  die "VM is up at $ip but SSH/cloud-init did not finish in time; check with '$0 ssh'"
}

cmd_ip() {
  virsh domifaddr "$VM_NAME" --source lease 2>/dev/null | awk '/ipv4/ {split($4,a,"/"); print a[1]; exit}'
}

cmd_ssh() { local ip; ip=$(cmd_ip); [ -n "$ip" ] || die "no IP; is the VM running?"; exec ssh "${SSH_OPTS[@]}" "$SSH_USER@$ip" "${@}"; }
cmd_start() { virsh start "$VM_NAME"; }
cmd_stop() { virsh shutdown "$VM_NAME"; }
cmd_status() { virsh dominfo "$VM_NAME"; echo; echo "ip: $(cmd_ip || echo none)"; }

cmd_purge() {
  echo "This removes the VM '$VM_NAME' and its disk under $POOL_DIR."
  read -r -p "Type the VM name to confirm: " answer
  [ "$answer" = "$VM_NAME" ] || die "aborted"
  virsh destroy "$VM_NAME" >/dev/null 2>&1 || true
  virsh undefine "$VM_NAME" --remove-all-storage
  echo "removed"
}

case "${1:-}" in
  create) cmd_create ;;
  ip) cmd_ip ;;
  ssh) shift; cmd_ssh "$@" ;;
  start) cmd_start ;;
  stop) cmd_stop ;;
  status) cmd_status ;;
  purge) cmd_purge ;;
  *) sed -n '2,20p' "$0"; exit 1 ;;
esac
