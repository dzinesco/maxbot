# MaxBot × Linux server — Setup runbook

One-time setup for MaxBot v2.0's per-Bot Linux VMs. After you finish
the steps below, MaxBot can spawn a fresh Ubuntu VM for each new Bot
and stream the desktop into the app.

This is the *infrastructure* setup. The MaxBot-side wiring (Settings
→ Computer tab, Bot creation flow, ComputerPanel) ships in v2.0.0.

## What you need first

- An Ubuntu 24.04 (or 22.04) Linux server, already installed and
  reachable from your Mac.
- SSH access as a non-root user (default: `tyler`) from the Mac.
  Tyler's SSH key is already saved.
- A `sudo` capability for that user (either passwordless via
  `/etc/sudoers.d/` or by typing the password when prompted).
- ~15 minutes.

The default placeholders in the commands below assume:
- `<SERVER>` → the server's IP or hostname (e.g. `192.168.0.49`)
- `<SERVER_USER>` → the SSH user (e.g. `tyler`)

If the server is **not** installed yet, install Ubuntu Server 24.04
LTS from the official ISO first: <https://releases.ubuntu.com/24.04/>
(the "Install (server)" image is ~2 GB). Default install options give
you a working `<SERVER_USER>` with SSH enabled — that's all we need.

## Step 0 — Verify the server is reachable over SSH

From your Mac:

```bash
SERVER=<your-server-ip>
SERVER_USER=<your-ssh-user>
ssh $SERVER_USER@$SERVER 'echo OK && uname -a && lsb_release -a 2>/dev/null && free -h | head -3 && df -h /'
```

Expect: `OK`, a Linux kernel line, an Ubuntu version line, plenty of
RAM, and plenty of free space. If this works, SSH is good and you can
proceed.

## Step 0.5 — Set up passwordless sudo (one-time, optional)

If your `<SERVER_USER>` already has passwordless sudo (via
`/etc/sudoers.d/`), skip this step. Otherwise, type the sudo password
once and let this step drop the NOPASSWD rule:

> **Note for macOS users (zsh):** `read -p` is a bash-ism. Use
> `printf` first.

```bash
SERVER=<your-server-ip>
SERVER_USER=<your-ssh-user>
printf "$SERVER_USER's sudo password on $SERVER: "
read -s TYLER_SUDO_PW
echo
ssh $SERVER_USER@$SERVER \
  "echo '$TYLER_SUDO_PW' | sudo -S -p '' bash -c '
    echo \"$SERVER_USER ALL=(ALL) NOPASSWD: ALL\" > /etc/sudoers.d/$SERVER_USER-nopasswd
    chmod 0440 /etc/sudoers.d/$SERVER_USER-nopasswd
    visudo -c -f /etc/sudoers.d/$SERVER_USER-nopasswd
  '" && unset TYLER_SUDO_PW
```

Verify (no password prompt expected):

```bash
ssh $SERVER_USER@$SERVER 'sudo -n id'
# Expect: uid=0(root) gid=0(root) groups=0(root)
```

Every step below uses `sudo -n` and runs unattended.

## Step 1 — Install the KVM stack

From your Mac:

```bash
SERVER=<your-server-ip>
SERVER_USER=<your-ssh-user>
ssh $SERVER_USER@$SERVER 'sudo apt update && \
  sudo apt install -y --no-install-recommends \
    qemu-kvm libvirt-clients libvirt-daemon-system \
    virtinst genisoimage qemu-utils openssh-server'
```

The `libvirt-daemon-system` package auto-creates the default `virbr0`
network (NAT + DHCP for VMs) and starts `libvirtd` on boot. Verify:

```bash
ssh $SERVER_USER@$SERVER 'ls -l /dev/kvm && sudo virsh net-list --all'
```

Expect: a `/dev/kvm` device, and a `default` network with state
`active` and `autostart` yes.

## Step 2 — Add user to the libvirt groups and create the maxbot directories

```bash
ssh $SERVER_USER@$SERVER 'sudo usermod -aG libvirt,kvm $SERVER_USER && \
  sudo mkdir -p /var/lib/maxbot/{base,vms,keys} && \
  sudo chown -R root:libvirt /var/lib/maxbot && \
  sudo chmod 2775 /var/lib/maxbot /var/lib/maxbot/vms /var/lib/maxbot/keys'
```

This adds the user to the `libvirt` + `kvm` groups (so `virsh` works
without sudo for read-only operations) and creates the
`/var/lib/maxbot/{base,vms,keys}` tree. The next SSH session picks up
the new groups automatically.

## Step 3 — Cache the Ubuntu 24.04 cloud image

```bash
ssh $SERVER_USER@$SERVER 'sudo mkdir -p /var/lib/maxbot/base && \
  [ ! -f /var/lib/maxbot/base/ubuntu-24.04-cloud.img ] && \
  sudo curl -L -o /var/lib/maxbot/base/ubuntu-24.04-cloud.img \
    https://cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-amd64.img && \
  sudo chmod 0644 /var/lib/maxbot/base/ubuntu-24.04-cloud.img'
```

Verify (idempotent — re-runs are no-ops):

```bash
ssh $SERVER_USER@$SERVER 'ls -lh /var/lib/maxbot/base/ubuntu-24.04-cloud.img'
```

Expect: ~600 MB, owned by root, world-readable.

## Step 3.5 — Create the shared folder (v3.5.0)

The `maxbotd` daemon owns the cross-Bot handoff folder at
`~/bots/_shared/`. This is a real host-side directory
(no overlay FS, no per-Bot VM mount) — three Mac-app
tools (`shared_read`, `shared_write`, `shared_list`)
read and write it on behalf of the Bots, mediated by the
daemon. The path-safety guard inside `tools::shared_fs`
rejects absolute paths, `..` segments, and symlinks that
resolve outside this folder.

Create it once on the server, owned by the user
`maxbotd` runs as (default: `tyler` if you ran the v3.1.0
`maxbotd-setup.md` walkthrough; the systemd unit's
`User=` is the source of truth):

```bash
SERVER=<your-server-ip>
SERVER_USER=<your-ssh-user>
ssh $SERVER_USER@$SERVER 'mkdir -p ~/bots/_shared && chmod 0775 ~/bots/_shared'
```

Verify (idempotent — re-runs are no-ops):

```bash
ssh $SERVER_USER@$SERVER 'ls -ld ~/bots/_shared'
# Expect: drwxrwxr-x ... <SERVER_USER> <SERVER_GROUP> ... bots/_shared
```

> **Future: a systemd `ExecStartPre=-` line in the
> `maxbotd.service` unit will own this path on first
> install** so the manual step above goes away. For
> v3.5.0, run the manual step once. Re-running is safe.

This is the directory the Mac app's `shared_write` tool
will land artifacts in (via the daemon). It is **not**
inside any per-Bot VM — it lives on the libvirt host
(`crispy`), owned by the daemon's user, with `0775`
so the daemon can write and any user with shell on
the host can read for debugging.

> **v3.7.1 — daemon-routed.** The Mac app's
> `shared_write` / `shared_read` / `shared_list`
> tools now route through the daemon's new
> `POST /shared` route, so the writes land here on
> `crispy` regardless of whether the calling Bot
> runs on the Mac or the daemon. The Mac's local
> `~/bots/_shared/` is a fallback only — it is
> read/written when the daemon is unreachable,
> with a `warn!` log and the `resolve_safe_path`
> guard still applied. Configure the Mac app's
> **Settings → General → maxbotd URL** to
> `http://crispy:8443` so the routed path is the
> default. See [`maxbotd-setup.md`](maxbotd-setup.md#v371--mac-app-routes-through-the-daemon)
> for the full setup flow and the manual `curl`
> smoke test.

## Step 4 — Deploy the `provision-vm.sh` script

MaxBot's Tauri side calls `/opt/maxbot/provision-vm.sh` on the server
for every new Bot. The full script content is in the project repo
under `src-tauri/scripts/provision-vm.sh` (deployed via the build's
`tauri.conf.json` resources, or install manually as below). To install
manually:

```bash
SERVER=<your-server-ip>
SERVER_USER=<your-ssh-user>

# 1. Save the script content to /tmp/provision-vm.sh on the Mac
#    (paste from src-tauri/scripts/provision-vm.sh in the repo).

# 2. scp it to the server and install
scp /tmp/provision-vm.sh $SERVER_USER@$SERVER:/tmp/provision-vm.sh
ssh $SERVER_USER@$SERVER 'sudo mkdir -p /opt/maxbot && \
  sudo mv /tmp/provision-vm.sh /opt/maxbot/provision-vm.sh && \
  sudo chmod 0755 /opt/maxbot/provision-vm.sh && \
  sudo chown root:root /opt/maxbot/provision-vm.sh && \
  sudo -n /opt/maxbot/provision-vm.sh; echo exit=$?'
```

The last command should exit with `1` (usage error, no args) — confirms
the script is callable as root without a password.

> **v3.7.2 note — lightdm + cloud-init is one-shot.** The provision
> script writes a `seed.iso` and the VM only ever runs cloud-init
> once. If you have a v3.0.x or v3.7.0 Bot provisioned before v3.7.2,
> its VM was built without `lightdm` and won't have a desktop on the
> QEMU framebuffer. **You must destroy + re-provision** that Bot to
> pick up the new lightdm + xfce4 + autologin config. The Brief has
> an opt-in `bootstrap-desktop.sh` helper as a one-liner for an
> existing VM (apt install the same packages + write the lightdm
> drop-in + `systemctl enable --now lightdm`), but a fresh provision
> is the supported path.

## Step 5 — End-to-end smoke test

Run a full provision → start → IP poll → destroy cycle, all from the
Mac via SSH. If this works, the server setup is done.

```bash
SERVER=<your-server-ip>
SERVER_USER=<your-ssh-user>
VM_NAME=maxbot-smoke-test-$$

# 1. Provision a real VM over SSH
ssh $SERVER_USER@$SERVER "sudo /opt/maxbot/provision-vm.sh \
  $VM_NAME 10 3072 \
  'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFAKEPUBLICKEY000000000000000000000000000000000000000 maxbot@max' \
  smoketest"

# 2. Wait ~30 seconds for the VM to boot and get a DHCP lease
sleep 30

# 3. Get the VM's IP via libvirt's built-in dnsmasq
ssh $SERVER_USER@$SERVER "sudo virsh net-dhcp-leases default | grep -A1 $VM_NAME"
# Expect: an entry with an IP like 192.168.122.X

# 4. Get the VNC port
ssh $SERVER_USER@$SERVER "sudo virsh vncdisplay $VM_NAME"
# Expect: ":0" (libvirt assigns display 0 → port 5900)

# 5. (Optional) Open a VNC viewer from your Mac to see the desktop:
#    open vnc://$SERVER_USER@$SERVER:5900
#    (or use Screen Sharing.app to see the XFCE desktop)

# 6. Cleanup
ssh $SERVER_USER@$SERVER "sudo virsh destroy $VM_NAME; \
  sudo virsh undefine $VM_NAME --remove-all-storage; \
  sudo rm -rf /var/lib/maxbot/vms/$VM_NAME"
```

If Step 3 reports an IP and Step 4 reports `:0`, the setup passes.
MaxBot's v2.0.0 code can now drive `provision-vm.sh` over SSH for
every Bot you create.

## What to paste into MaxBot

Once Step 5 passes, open MaxBot → Settings → Computer and fill in:

| Setting                       | Value                                           |
|-------------------------------|-------------------------------------------------|
| Server host                   | `192.168.0.49` (or your `<SERVER>`)            |
| Server SSH user               | `tyler` (or your `<SERVER_USER>`)              |
| Server SSH key id             | (leave blank for now — uses your Mac's default `~/.ssh/id_ed25519`) |
| VNC local port range          | `5900–5999` (default)                          |
| Default per-Bot RAM           | `2048` MB (2 GB; ~27 concurrent by RAM)         |
| Default per-Bot disk          | `10` GB (~49 concurrent by disk, with 491 GB free) |
| Computer passphrase           | (set once — encrypts per-Bot SSH keypairs)      |

The passphrase is the only secret. It's stored locally in
`~/Library/Application Support/com.maxbot.app/maxbot.sqlite`.

Click **Test connection** to confirm MaxBot can talk to the server.

## Troubleshooting

**`Permission denied (publickey)` from `ssh tyler@<SERVER>`.**
Your SSH key isn't on the server yet. From the Mac:
```bash
ssh-copy-id tyler@<SERVER>
```

**`sudo: a password is required`.**
Step 0.5 wasn't run, or it failed. Re-run Step 0.5.

**`ls: cannot access '/dev/kvm': No such file or directory`.**
The server's CPU doesn't expose virtualization, or virtualization is
disabled in the BIOS. Check:
```bash
ssh tyler@<SERVER> 'egrep -c "(vmx|svm)" /proc/cpuinfo'
# Expect: a non-zero number. If 0, virtualization isn't exposed.
```

**`virsh: command not found`.**
The `libvirt-clients` package wasn't installed. Re-run Step 1.

**`virsh net-list --all` returns empty.**
The `libvirt-daemon-system` package didn't auto-start the default
network. Start it manually:
```bash
ssh tyler@<SERVER> 'sudo virsh net-start default && sudo virsh net-autostart default'
```

**The smoke test VM gets no IP via `virsh net-dhcp-leases default`.**
`virbr0` doesn't have DHCP working, or the VM hasn't booted yet. Wait
30 seconds and retry. If still no lease, check the VM console:
```bash
ssh tyler@<SERVER> "sudo virsh console $VM_NAME"
```
(Press `<Enter>` to wake the console; `<Ctrl>+]` to exit.)

**`provision-vm.sh: line N: qemu-img: command not found`.**
`qemu-utils` wasn't installed. Re-run Step 1.

## What gets installed on each Bot VM

The base image is a **bare Ubuntu 24.04 cloud image with cloud-init
ready**. It does not yet have XFCE or a desktop. MaxBot's
Bot-provisioning step (in `provision-vm.sh`, called for every Bot)
injects the desktop packages per-VM via cloud-init's `runcmd` when
the VM first boots — that keeps the base image small and the
provisioning step flexible. The per-VM install includes:
`lightdm`, `xfce4`, `xfce4-goodies`, `qemu-guest-agent`,
`openssh-server`, **`chromium-browser`**, **`xdotool`**, and
**`scrot`** (the last three are v3.2.0 — they back the
`vm_computer_use` tool that drives the Bot's own browser and
desktop over SSH).

> **v3.7.2 change:** `x11vnc` and `tigervnc-standalone-server` are
> no longer installed per-Bot. The v3.0.x noVNC-over-RFB path was
> dropped in v3.7.2 because the Tauri webview (WKWebView) didn't
> render noVNC's canvas reliably, and the v3.7.2 path uses
> `virsh screenshot` (host-side QEMU framebuffer poll) for Preview
> and `ssh -L` + macOS Screen Sharing for Takeover — no in-VM VNC
> server required. The `lightdm + xfce4` combo is what makes the
> desktop visible to the host-side screenshot.

> **Re-provisioning note (v3.2.0):** cloud-init only runs ONCE per
> VM, at first boot. If you have existing v3.0.x or v3.1.x VMs
> they will NOT pick up the new packages automatically — destroy
> them and provision a fresh VM to get the v3.2.0 toolchain
> (`chromium-browser`, `xdotool`, `scrot`). The `apt install` step
> in cloud-init's `packages:` block is idempotent on a fresh image
> (apt short-circuits when the package is already at the right
> version), so a destroyed-and-reprovisioned VM does not need any
> extra care.
