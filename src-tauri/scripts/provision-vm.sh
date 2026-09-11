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
# v3.7.5: patch the libvirt domain XML post-create to inject
# `<qemu:commandline>` with `-vnc 127.0.0.1:0,password=off,to=5999`.
# macOS Screen Sharing prompts for a VNC password on Takeover because
# QEMU 9.x defaults to VNC's DES challenge even with no password set.
# The v2.0.3 fix relied on QEMU's default (no password → no auth), but
# the default changed in QEMU 9.x / Ubuntu 25.10. We tried:
#   (a) `auth=none` via virt-install's `--graphics` — rejected by
#       libvirt 11.6.0 ("Unknown --graphics options: ['auth']").
#   (b) `auth=none` via qemu:commandline — rejected by QEMU 9.x
#       ("Invalid parameter 'auth'").
#   (c) `password=off` via qemu:commandline — accepted by QEMU;
#       RFB handshake advertises VNC_AUTH_NONE only (verified
#       end-to-end on maxbot-bot-8eb75d73 on 2026-09-10).
# The qemu:commandline is appended after virt-install, via
# `virsh dumpxml | python3 | virsh define /dev/stdin`. The
# virt-install uses `--noreboot` so the VM doesn't start until
# AFTER the patch lands — QEMU doesn't reload VNC config on the
# fly, so if the VM started first, the new auth would be ignored
# on first boot. The `virsh start` at the end of the patch block
# is what actually boots the VM, with the patched XML in place.
# Trust model unchanged: SSH-gated tunnel, libvirt loopback bind,
# only the MaxBot process ever has SSH access. Removes the
# password prompt so the 2FA walkthrough (the rest of v3.7.5)
# doesn't have a "hunt the password" step before it can start.
# Existing VMs need Destroy + re-provision to pick up the new auth.
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
# v3.7.16: 6th positional arg is the VNC display number
# (0..=99 → TCP port 5900..=5999). The Mac side picks a
# free display by reading the `computers.vnc_port` rows
# and passes it explicitly so multiple Bots get sequential
# ports instead of all colliding on 5900. Defaults to 0
# when called by hand (so existing single-VM usage keeps
# working). The previous v3.7.5 hard-coded `127.0.0.1:0`
# is replaced with `127.0.0.1:${VNC_DISPLAY}`.
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
# v3.7.16: VNC display number. Defaults to 0 so a
# hand-call (e.g. while debugging) keeps the v3.7.5
# behavior of binding to 127.0.0.1:0.
VNC_DISPLAY="${6:-0}"

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
  # v3.7.11 — OCR-augmented screenshots. After every
  # `screenshot()` call, the tool runs
  # `tesseract /tmp/maxbot-screen.png -` over the same
  # SSH pool to extract the visible text. The model
  # then gets both the PNG and the OCR text in the
  # tool result, so it can target buttons / links by
  # label instead of guessing coordinates from pixels.
  # `tesseract-ocr` is the engine; `tesseract-ocr-eng`
  # (the English language data) is a recommended
  # dependency and gets pulled in automatically — the
  # Bot's UI is overwhelmingly English-language so
  # that's sufficient for v3.7.11. Adding other
  # languages (e.g. `tesseract-ocr-spa`) is a
  # future slice if Tyler starts running Bots against
  # non-English pages. Re-provision existing VMs
  # to pick this up (cloud-init only runs once per VM
  # at first boot).
  - tesseract-ocr
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
  # v3.7.10: pre-create the chromium profile dir
  # at the explicit path the bot's `vm_computer_use`
  # launches against (--user-data-dir=...). The
  # path is on the qcow2's `/home/bot/.config/`,
  # so cookies and login sessions survive
  # reboots. cloud-init's `runcmd:` runs as root,
  # so chown to `bot:bot` is required — without
  # it, chromium (launched by the `bot` user) would
  # create the dir as root on first launch and the
  # second launch (after a cookie or extension
  # write) would fail with EACCES. Mode 0700 so
  # other users on the VM can't read the bot's
  # cookies. The dir is also useful when the user
  # takes over the panel — they can inspect
  # `/home/bot/.config/chromium-maxbot/Default/`
  # to see cookies, history, and the on-disk
  # extension list.
  - mkdir -p /home/bot/.config/chromium-maxbot
  - chown -R bot:bot /home/bot/.config/chromium-maxbot
  - chmod 700 /home/bot/.config/chromium-maxbot
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
#    NO libvirt VNC. We use `--graphics none` so QEMU does NOT start
#    a libvirt-managed VNC server with default DES auth. The only
#    VNC server is the one injected via `<qemu:commandline>` below
#    (`-vnc 127.0.0.1:<VNC_DISPLAY>,password=off,to=5999`) with no
#    auth. v3.7.16: the `0` is now the `VNC_DISPLAY` shell var
#    (the Mac side picks a free display 0..=99, default 0 for
#    hand-calls); v3.7.5 hard-coded it.
#
#    Why this matters: if BOTH libvirt's VNC and qemu:commandline's
#    VNC run, QEMU starts two VNC servers — one on the libvirt
#    port (default DES challenge) and one on a bumped port (no
#    auth). `virsh vncdisplay` only knows about the libvirt one,
#    so the Mac app's SSH tunnel goes to the wrong (auth-required)
#    VNC. macOS Screen Sharing then prompts for a password. With
#    `--graphics none`, only the no-auth VNC exists, and the
#    Mac app's `poll_for_vnc_port` fallback is gone (v3.7.16) —
#    the port is now `5900 + VNC_DISPLAY`, picked up-front on
#    the Mac side.
#
#    `--noreboot` is load-bearing: virt-install's default is to
#    define + start the VM in one step, but the qemu:commandline
#    patch below has to land in the XML BEFORE the QEMU process
#    starts, otherwise QEMU boots with the old args (QEMU doesn't
#    reload VNC config on the fly). With `--noreboot`, the
#    `virsh start` after the patch is what actually boots the
#    VM, with the patched XML in place.
#
#    v3.7.5: the libvirt domain XML gets a `<qemu:commandline>`
#    block with `-vnc 127.0.0.1:<VNC_DISPLAY>,password=off,to=5999`.
#    This disables VNC password auth so macOS Screen Sharing
#    connects without prompting. Why `password=off` via
#    qemu:commandline:
#      - libvirt 11.6.0 doesn't accept `auth` as a `<graphics>`
#        attribute (only `vnc` and `sasl` are valid).
#      - QEMU 9.x on Ubuntu 25.10 rejects `auth=none` as an
#        "Invalid parameter" on the `-vnc` option.
#      - QEMU 9.x accepts `password=off`. RFB handshake
#        advertises VNC_AUTH_NONE only (verified end-to-end
#        against maxbot-bot-8eb75d73 on 2026-09-10).
#      - The `to=5999` lets QEMU pick any free port in 5900-5999
#        (v3.7.16: still useful as a safety net if the display
#        number we pass happens to be busy).
#    Trust model unchanged: SSH-gated tunnel + libvirt loopback
#    bind is the only real gate. macOS Screen Sharing no longer
#    prompts.
#
#    Existing VMs (provisioned before this fix) need Destroy +
#    re-provision (or `virsh shutdown` + start) to pick up the
#    new auth — see the v3.7.5 entry in CHANGELOG.md.
virt-install \
  --name "$VM_NAME" \
  --memory "$RAM_MB" \
  --vcpus 2 \
  --disk "$VM_DIR/disk.qcow2",format=qcow2 \
  --disk "$VM_DIR/seed.iso",device=cdrom \
  --import \
  --os-variant ubuntu24.04 \
  --network bridge=virbr0,model=virtio \
  --graphics none \
  --noreboot \
  --noautoconsole

# 4a. Patch the domain XML to add a `<video>` element (so the
#     VM has a virtual VGA that `virsh screenshot` can poll)
#     AND a `<qemu:commandline>` block with
#     `-vnc 127.0.0.1:<VNC_DISPLAY>,password=off,to=5999` (so
#     the VNC server is no-auth and on the Mac-picked port).
#
#     The `<video>` element is required because `--graphics none`
#     tells libvirt not to add any display device. Without an
#     explicit `<video>`, `virsh screenshot` fails with
#     "no screens to take screenshot from" (verified 2026-09-10
#     on maxbot-bot-f0e7f365). The `<video>` element goes
#     through libvirt's schema and gets the right PCI slot; using
#     qemu:commandline's `-vga std` instead collides with
#     libvirt's pcie-root-port slot allocation
#     ("PCI: slot 1 function 0 not available for pcie-root-port,
#     in use by VGA").
#
#     The VNC server stays via qemu:commandline because libvirt
#     11.6 doesn't accept `auth` as a `<graphics>` attribute
#     (only `vnc` and `sasl` are valid) and QEMU 9.x rejects
#     `auth=none` on the `-vnc` option. The working path is
#     `password=off` via qemu:commandline. v3.7.16: the
#     `<VNC_DISPLAY>` slot is the Mac-picked display number
#     (0..=99), so the second Bot gets port 5901 instead of
#     colliding on 5900.
#
#     Uses Python (always present on Ubuntu) for the multi-line
#     regex; sed can't handle the namespace + the <video>
#     injection cleanly.
virsh dumpxml "$VM_NAME" | python3 -c "
import re, sys
x = sys.stdin.read()
x = re.sub(r'<qemu:commandline>.*?</qemu:commandline>', '', x, flags=re.DOTALL)
x = re.sub(r'<video>.*?</video>', '', x, flags=re.DOTALL)
x = re.sub(r'<video[^/]*/>', '', x)
x = re.sub(r\"\"\"\\sxmlns:qemu=['\\\"][^'\\\"]+['\\\"]\"\"\", '', x)
if 'xmlns:qemu' not in x:
    x = x.replace(
        '<domain type=',
        \"<domain xmlns:qemu='http://libvirt.org/schemas/domain/qemu/1.0' type=\",
        1,
    )
# Add <video><model type='vga'/></video> right before the closing
# </devices> tag, so libvirt allocates a PCI slot for the VGA
# without colliding with the pcie-root-port it allocated.
x = x.replace('</devices>', \"<video><model type='vga'/></video></devices>\", 1)
# v3.7.16: substitute the VNC display number from argv[1]
# (the Mac side picks a free display 0..=99; defaults to 0
# at the top of the script for hand-calls). The
# `to=5999` knob still gives QEMU room to pick the next
# free port if the exact display is busy for any reason.
vnc_display = sys.argv[1] if len(sys.argv) > 1 else '0'
qemu_block = (
    '<qemu:commandline>'
    \"<qemu:arg value='-vnc'/>\"
    \"<qemu:arg value='127.0.0.1:' + vnc_display + ',password=off,to=5999'/>\"
    '</qemu:commandline>'
    '</domain>'
)
x = x.replace('</domain>', qemu_block, 1)
sys.stdout.write(x)
" "$VNC_DISPLAY" | virsh define /dev/stdin

# 4b. Start the VM now that the qemu:commandline patch is in the
#     persistent config. `--noreboot` above left it shut off; this
#     is where it actually boots, with QEMU picking up the
#     `-vnc 127.0.0.1:<VNC_DISPLAY>,password=off,to=5999` arg from
#     the qemu:commandline block.
virsh start "$VM_NAME"

# 5. Return the libvirt domain name
echo "$VM_NAME"
