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
# discarded — the VNC client never received it, so the RFB
# handshake always failed with "VNC security handshake failed" and
# the console area silently rendered blank. With `-nopw`, the VNC
# port is open to anyone on the server's loopback, but only the
# MaxBot process ever has SSH access there, so the security model
# is unchanged. The 5th positional argument is preserved (and
# ignored) for API compatibility with the Rust side.
#
# v2.1: ensure sshd is up + the bot user is authorized BEFORE
# the long packages: install runs. Without this, the cold-cache
# smoke test (provision_e2e_against_crispy) saw port 22 refuse
# connections for 2+ minutes after the IP lease. Root cause: the
# noble cloud image ships /etc/ssh/sshd_config but NO host keys.
# cc_ssh normally generates them, but it runs AFTER bootcmd --
# so when our bootcmd does systemctl enable --now ssh, sshd's
# ExecStartPre (sshd -t) aborts with "no hostkeys available",
# and because ssh.service is Type=notify, systemctl then
# blocks forever waiting for READY=1. We tried a host-key
# pre-generation, but the underlying deadlock with Type=notify
# still bites when the apt-get install of xfce4
# races with the systemd notification. The fix:
#   1. bootcmd generates host keys with ssh-keygen -A BEFORE
#      trying to start sshd, so sshd -t passes.
#   2. bootcmd pre-creates bot + drops the authorized_keys
#      so SSH works the moment sshd binds port 22 (no wait for
#      cc_users_groups, which is in cloud_config_modules and
#      gets stuck behind the apt install of xfce4).
#   3. bootcmd starts /usr/sbin/sshd DIRECTLY (no systemd
#      Type=notify, no systemctl --now). Port 22 binds in
#      milliseconds, independent of how long the apt install
#      of xfce4 / openssh-server takes.
#   4. The original users: block is kept (cc_users_groups is
#      idempotent and re-confirms the same key).
#   5. runcmd: ends with systemctl enable ssh (so systemd
#      owns it on the next reboot) and systemctl try-restart
#      ssh (safety net for if the openssh-server package
#      install ever disrupts the manually-started sshd).
#
# Implementation note: the user-data heredoc uses plain <<EOF
# (NOT <<'EOF') so $SSH_PUB and $VM_NAME still expand, but
# that means backticks in comments would be evaluated as
# command substitution. Don't add any.

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
bootcmd:
  # Pre-create the "bot" user with sudo + the SSH key so SSH
  # is authorized as soon as sshd binds port 22. bootcmd
  # runs before any cloud_init_modules entry, so this is done
  # well before cc_ssh / cc_users_groups / package install.
  # The downstream users: block is idempotent and just
  # re-confirms the same key.
  - id bot >/dev/null 2>&1 || useradd -m -s /bin/bash -G sudo,adm bot
  - mkdir -p /home/bot/.ssh
  - chmod 700 /home/bot/.ssh
  - echo '$SSH_PUB' > /home/bot/.ssh/authorized_keys
  - chmod 600 /home/bot/.ssh/authorized_keys
  - chown -R bot:bot /home/bot/.ssh
  # Generate host keys BEFORE trying to start sshd. The noble
  # cloud image ships /etc/ssh/sshd_config but NO host keys
  # (ssh-keygen -A normally runs inside cc_ssh, but that
  # module comes after this bootcmd). Without host keys,
  # sshd -t (ExecStartPre) refuses to start.
  - if [ ! -f /etc/ssh/ssh_host_ed25519_key ]; then ssh-keygen -A; fi
  # Make sure the privsep dir systemd will create is in place
  # (ssh.service RuntimeDirectory=sshd would create it
  # later, but we start sshd before that).
  - mkdir -p /run/sshd && chmod 0755 /run/sshd
  # Start sshd directly. systemctl enable --now ssh deadlocks
  # in this image because ssh.service is Type=notify and
  # systemd's notify machinery races with the apt-get install
  # of xfce4 that the packages: block kicks off
  # right after bootcmd returns. Starting the binary
  # directly (no systemd notification) gives us a working
  # port 22 within milliseconds, independent of how long
  # the apt install takes.
  - /usr/sbin/sshd
users:
  - name: bot
    groups: [sudo, adm]
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    ssh_authorized_keys:
      - $SSH_PUB
packages:
  # v3.7.2: the `packages:` block now installs lightdm +
  # xfce4 only. The v3.0.x path tunneled a separate in-VM
  # VNC server on `:1` for the in-app console, but the
  # Tauri webview (WKWebView) didn't render its canvas
  # path reliably, and the two-screen mismatch (QEMU
  # virtual VGA + a separate VNC display on `:1`) is what
  # kept the preview showing a created-but-invisible VM.
  # The v3.7.2 path: the in-app preview is a
  # `virsh screenshot` poll on the QEMU virtual VGA, and
  # takeover uses macOS `Screen Sharing` over the existing
  # SSH `-L` tunnel — no in-VM VNC server needed.
  #
  # LightDM + xfce4 autologin brings up a real desktop
  # on the QEMU display so `virsh screenshot` returns
  # a real frame (not a text console / black canvas)
  # once cloud-init finishes.
  - lightdm
  - xfce4
  - xfce4-goodies
  - dbus-x11
  - qemu-guest-agent
  - openssh-server
  # v3.2.0 — in-VM Computer Use. The `vm_computer_use` tool
  # (src-tauri/src/tools/vm_computer_use.rs) drives the VM's own
  # browser + desktop via these CLI tools:
  #   - `chromium-browser` is the Bot's browser, opened via
  #     `chromium --no-sandbox <url>`. The `--no-sandbox` is
  #     needed because we're running as root inside the VM
  #     (cloud-init's `bootcmd` block creates the `bot` user
  #     and the per-Bot Computer runs as `bot`, but chromium
  #     still needs `--no-sandbox` for SUID sandboxes that
  #     conflict with the in-VM display server).
  #   - `xdotool` provides `mousemove x y`, `click 1`, `type`,
  #     and `key` for synthetic input. The tool shells out to
  #     these via the existing SshPool.
  #   - `scrot` is a tiny CLI screenshot tool (60KB). The tool
  #     calls `scrot -z /tmp/screen.png` and SCPs the result
  #     back as the post-action screenshot. `xwd` would also
  #     work but requires piping through `convert`.
  # These three packages are added to the `packages:` block
  # (not the `runcmd:` block) so cloud-init's `package_update`
  # + `package_install` modules handle them as a single apt
  # transaction. Re-provision existing VMs to pick them up —
  # cloud-init only runs once per VM (at first boot).
  - chromium-browser
  - xdotool
  - scrot
runcmd:
  # v3.7.2: lightdm + xfce autologin. The noble cloud
  # image doesn't ship a display manager — without
  # this, the QEMU virtual VGA stays at a text console
  # and `virsh screenshot` returns a black frame.
  # The drop-in picks XFCE as the user session and
  # autologs in as `bot` so the desktop is up by the
  # time the first screenshot is taken.
  - mkdir -p /etc/lightdm/lightdm.conf.d
  - |
      printf '%s\n' \
        '[Seat:*]' \
        'autologin-user=bot' \
        'autologin-user-timeout=0' \
        'user-session=xfce' \
        > /etc/lightdm/lightdm.conf.d/50-maxbot.conf
  - echo /usr/sbin/lightdm > /etc/X11/default-display-manager
  - systemctl set-default graphical.target
  - systemctl enable lightdm
  - systemctl enable --now qemu-guest-agent
  # Safety net: re-start sshd in case the openssh-server
  # package upgrade in the packages: block (or any other
  # service install) left it in a bad state. systemctl
  # enable ssh (without --now) wires it up for future
  # reboots; the systemd unit will then take over from
  # the manually-started sshd via ExecStartPre on a
  # normal start.
  - systemctl enable ssh
  - systemctl try-restart ssh
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
