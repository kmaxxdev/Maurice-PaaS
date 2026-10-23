#!/usr/bin/env bash
# Build the maurice binary locally, copy it into the dev VM and run it there as
# root. Host and VM share the same glibc (both Ubuntu 26.04), so no cross build.
#
# Usage:
#   scripts/vm-run.sh host selftest --image docker.io/library/postgres:16
#   scripts/vm-run.sh tier
set -euo pipefail

HERE=$(cd "$(dirname "$0")/.." && pwd)
SSH_KEY="${MAURICE_VM_SSH_KEY:-$HOME/.ssh/id_ed25519_kmaxx}"
VM_USER="${MAURICE_VM_USER:-dev}"
VM_IP="${MAURICE_VM_IP:-$("$HERE/scripts/dev-vm.sh" ip 2>/dev/null || true)}"
[ -n "$VM_IP" ] || { echo "error: VM address unknown; set MAURICE_VM_IP" >&2; exit 1; }

SSH=(ssh -i "$SSH_KEY" -o IdentitiesOnly=yes -o BatchMode=yes "$VM_USER@$VM_IP")

(cd "$HERE" && cargo build -q -p maurice)
scp -q -i "$SSH_KEY" -o IdentitiesOnly=yes "$HERE/target/debug/maurice" "$VM_USER@$VM_IP:/tmp/maurice"
"${SSH[@]}" "sudo install -m 0755 /tmp/maurice /usr/local/bin/maurice && sudo ln -sf maurice /usr/local/bin/marik && sudo ln -sf maurice /usr/local/bin/maurice-odysseus-stella"
exec "${SSH[@]}" -t "sudo RUST_LOG=\${RUST_LOG:-info} /usr/local/bin/maurice $(printf '%q ' "$@")"
