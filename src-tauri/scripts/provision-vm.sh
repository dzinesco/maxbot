#!/usr/bin/env bash
# MaxBot per-Bot VM provisioning script.
# Called by the Tauri side via SSH (e.g.
#   ssh tyler@<SERVER> 'sudo /opt/maxbot/provision-vm.sh <vm_name> <disk_gb> <ram_mb> <ssh_pubkey> [unused]'
# ).
#
# Idempotent: if a VM with the same name already exists, libvirt will
# error and the script exits non-zero — caller is expected to handle
# that.
#
# Output: prints the libvirt domain name on success.
#
# v2.0.3: removed VNC password requirement. The VNC port is only
# reachable from the server itself (libvirt binds to 127.0.0.1) and
# the Tauri side bridges to it over an SSH tunnel that already
# requires the user's SSH key. The VNC password was a second secret
# that the Rust side generated, sent to the script, and then
# discarded — the noVNC client never received it, so the RFB
# handshake always failed with "VNC security handshake failed" and
# the console area silently rendered blank. With `-nopw`, the VNC
# port is open to anyone on the server's loopback, but only the
# MaxBot process ever has SSH access there, so the security model
# is unchanged. The 5th positional argument is preserved (and
# ignored) for API compatibility with the Rust side.

set -euo pipefail

VM_NAME="$1"
DISK_GB="$2"
RAM_MB="$3"
SSH_PUB="$4"
# VNC_PASSWORD="$5"  # ignored — see header comment

# 1. Download a base image (Ubuntu 24.04 cloud) if not cached
BASE_IMAGE="/var/lib/maxbot/base/ubuntu-24.04-cloud.img"
if [ ! -f "$BASE_IMAGE" ]; then
  mkdir -p "$(dirname "$BASE_IMAGE")"
  curl -L -o "$BASE_IMAGE" \
    https://cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-amd64.img
fi

# 2. Create a qcow2 overlay for the new VM
VM_DIR="/var/lib/maxbot/vms/$VM_NAME"
mkdir -p "$VM_DIR"
qemu-img create -f qcow2 -b "$BASE_IMAGE" -F qcow2 "$VM_DIR/disk.qcow2" "${DISK_GB}G"

# 3. Generate cloud-init ISO with the SSH key. NoCloud
#    requires BOTH user-data AND meta-data on the ISO
#    (meta-data can be empty but the file must exist).
USER_DATA="$VM_DIR/user-data"
META_DATA="$VM_DIR/meta-data"
cat > "$USER_DATA" <<EOF
#cloud-config
users:
  - name: bot
    groups: [sudo, adm]
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    ssh_authorized_keys:
      - $SSH_PUB
packages:
  - xfce4
  - xfce4-goodies
  - x11vnc
  - tigervnc-standalone-server
  - qemu-guest-agent
  - openssh-server
runcmd:
  - systemctl set-default graphical.target
  - systemctl enable --now qemu-guest-agent
  - sudo -u bot mkdir -p /home/bot/.vnc
  - sudo -u bot bash -c 'echo "x11vnc -display :1 -forever -nopw -bg -o /home/bot/.vnc/x11vnc.log" > /home/bot/.vnc/xstartup'
  - sudo -u bot bash -c '( crontab -l 2>/dev/null | grep -v x11vnc; echo "@reboot x11vnc -display :1 -forever -nopw -bg -o /home/bot/.vnc/x11vnc.log" ) | crontab -'
EOF
# Minimal NoCloud meta-data (required by cloud-init).
cat > "$META_DATA" <<EOF
instance-id: $VM_NAME
local-hostname: ${VM_NAME//_/-}
EOF
genisoimage -output "$VM_DIR/seed.iso" -volid cidata -joliet -rock "$USER_DATA" "$META_DATA"

# 4. Define the libvirt domain.
#    VNC is bound to 127.0.0.1 — the Mac tunnels it via `ssh -L` in
#    the Tauri VNC proxy (see src-tauri/src/computer/vnc.rs).
virt-install \
  --name "$VM_NAME" \
  --memory "$RAM_MB" \
  --vcpus 2 \
  --disk "$VM_DIR/disk.qcow2",format=qcow2 \
  --disk "$VM_DIR/seed.iso",device=cdrom \
  --import \
  --os-variant ubuntu24.04 \
  --network bridge=virbr0,model=virtio \
  --graphics vnc,listen=127.0.0.1,port=-1 \
  --noautoconsole

# 5. Return the libvirt domain name
echo "$VM_NAME"
