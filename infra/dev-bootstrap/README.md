# MaxBot dev VM bootstrap

One-shot setup for a fresh Ubuntu 24.04 Bot VM to add the dev
apps Tyler wanted "set up correctly" — Node 22 LTS, Postgres 16,
code-server, plus a `dev` user with the same SSH key as `bot`.
Idempotent. Safe to re-run.

## What's in here

- `dev-bootstrap.sh` — full bootstrap (apt update + core CLI + Node
  22 via NodeSource + Postgres 16 via PGDG + code-server + dev
  user). Run this on a freshly-provisioned Bot VM.
- `dev-bootstrap-cont.sh` — continuation script (skips the
  apt-installed core tools; useful for re-runs after a partial
  install or to add only the higher-level tools).
- `qga_sh.py` — Python helper: run a shell command in a Bot VM
  via `virsh qemu-agent-command` and return stdout/stderr
  decoded. Used for the install + key-injection paths.
- `qga_add_key.sh` — Bash wrapper around QGA that adds the
  current user's SSH pubkey to a Bot's `/home/bot/.ssh/authorized_keys`
  via `virsh qemu-agent-command` + `guest-exec`. Used to get SSH
  access to a freshly-provisioned VM before the Console panel
  has had a chance to run `install_default_key_via_qga`.
- `patch_vnc_auth.py`, `add_vga.py`, `add_video.py`,
  `strip_libvirt_graphics.py` — libvirt-domain-XML patchers
  for the VNC no-auth fix (see v3.7.5 in CHANGELOG). Used to
  migrate running VMs to the new XML without Destroy+re-provision.
- `test_vnc_auth.py` — Python RFB handshake tester. Sends
  `RFB 003.008\n`, prints the security types the server
  advertises, picks type 1 (VNC_AUTH_NONE), and reports whether
  the server accepts the no-auth connection. Run on crispy
  against a known VNC port to verify the no-auth path works
  end-to-end (e.g. `python3 test_vnc_auth.py 127.0.0.1 5900`).

## Why a separate infra/ folder

These scripts are operational: they're not part of the Tauri app
or the libvirt domain XML; they run on the host (`crispy`) or
via QGA inside the VM during the post-provision dance. They
should live with the codebase for reproducibility (next time
someone wipes the SQLite and provisions a fresh Bot, this is
what you re-run), not buried in /tmp on someone's laptop.

## Run order for a fresh Bot VM

1. **Provision the VM from the MaxBot app** (click Provision on
   a new Bot, or `+ New Bot` + Provision). This is the normal flow.
2. **Wait for `state === "running"`** in the Computer panel
   (5–10 minutes; cloud-init + apt install).
3. **Add Tyler's SSH pubkey to the Bot** so the host can SSH in
   directly:

   ```bash
   scp infra/dev-bootstrap/qga_sh.py tyler@crispy:/tmp/qga_sh.py
   ssh tyler@crispy 'sudo -n /tmp/qga_sh.py <bot-domain> "id"'
   # (QGA might not be up yet on a fresh VM; if "Guest agent is
   # not responding", wait 30s and try again, or Destroy + re-provision.)
   ssh tyler@crispy "sudo -n /tmp/qga_sh.py <bot-domain> '/tmp/qga_add_key.sh <bot-domain> /home/<user>/.ssh/id_ed25519.pub'"
   ```

4. **Push and run the bootstrap** via QGA:

   ```bash
   scp infra/dev-bootstrap/dev-bootstrap.sh tyler@crispy:/tmp/dev-bootstrap.sh
   ssh tyler@crispy "sudo -n bash -c 'B64=\$(base64 -w0 /tmp/dev-bootstrap.sh); /tmp/qga_sh.py <bot-domain> \"echo \$B64 | base64 -d > /tmp/dev-bootstrap.sh && chmod +x /tmp/dev-bootstrap.sh && /tmp/dev-bootstrap.sh\"'"
   ```

5. **Verify**:

   ```bash
   ssh tyler@crispy "sudo -n /tmp/qga_sh.py <bot-domain> 'node --version; npm --version; psql --version; code-server --version; id bot dev'"
   ```

## Notes

- The bootstrap does **not** touch the existing `lightdm +
  xfce4 + chromium + xdotool + scrot` runtime from the
  provision script. It's purely additive.
- code-server binds to `127.0.0.1:8080` by default; expose via
  the Takeover SSH tunnel pattern (`ssh -L 8080:127.0.0.1:8080`)
  to access from the Mac.
- Postgres 16 listens on `127.0.0.1:5432` (libvirt bridge
  default). To access from the Mac, either SSH-tunnel
  (`ssh -L 5432:127.0.0.1:5432`) or expose via
  `/etc/postgresql/16/main/postgresql.conf` `listen_addresses`.
- The `dev` user has Tyler's SSH key installed; `su - dev` from
  the `bot` shell works without a password.

## v3.7.8: bochs-drm / linux-modules-extra (REQUIRED for graphical boot)

If the VM boots to **tty1** (a text-mode login prompt) instead
of the LightDM greeter, the most likely cause is a missing
`bochs-drm` kernel module. The QEMU virtual VGA (PCI `1234:1111`)
needs the `bochs` driver, which lives in
`linux-modules-extra-$(uname -r)`. The Ubuntu 24.04 noble cloud
image only ships `linux-image-virtual`, not the modules-extra;
the install is required for LightDM + XFCE to come up.

The fix (already in `dev-bootstrap.sh` step 1 + step 6 as of
v3.7.8):

```bash
DEBIAN_FRONTEND=noninteractive apt-get install -y linux-modules-extra-$(uname -r)
echo bochs > /etc/modules-load.d/bochs.conf
modprobe bochs
systemctl restart lightdm
```

`lightdm` shows as `indirect` under `is-enabled` — that's
correct (it's pulled in by `graphical.target`, which is the
default). `systemctl enable lightdm` returns a "no
installation config" warning for the same reason; do not
chase it.
