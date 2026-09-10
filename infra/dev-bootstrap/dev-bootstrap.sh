#!/usr/bin/env bash
# MaxBot dev VM bootstrap. Run once on a fresh Ubuntu 24.04
# Bot VM to add the dev apps Tyler wanted "set up correctly":
#   - core CLI tools: git, curl, jq, build-essential, ca-certificates,
#     gnupg, software-properties-common, apt-transport-https
#   - Node.js 22 LTS via NodeSource
#   - PostgreSQL 16 via PGDG (apt)
#   - code-server (browser VS Code) via the official install script
#   - a `dev` user (mirrors `bot`: passwordless sudo, ssh auth)
#
# Idempotent: safe to re-run.

set -euo pipefail

export DEBIAN_FRONTEND=noninteractive

# 1. core apt packages
apt-get update -qq
apt-get install -y -qq \
    git \
    curl \
    wget \
    jq \
    ca-certificates \
    gnupg \
    lsb-release \
    software-properties-common \
    apt-transport-https \
    build-essential \
    pkg-config \
    libssl-dev \
    unzip \
    htop \
    tmux \
    ripgrep \
    fd-find \
    # v3.7.8 fix: linux-modules-extra-$(uname -r) is
    # REQUIRED for the QEMU virtual VGA (PCI 1234:1111)
    # to work. The bochs-drm driver lives in
    # linux-modules-extra — without it, Xorg falls
    # back to fbdev/vesa and both fail (no
    # /dev/dri/card0, /dev/fb0 missing, vesa "No
    # matching modes"), so LightDM exits 1 on every
    # boot and the VM stays at tty1. The cloud image
    # only ships linux-image-virtual, not the modules-
    # extra; installing it on first bootstrap is
    # simpler than chasing it down at first lightdm
    # failure. (Discovered 2026-09-10: Robot was
    # left at tty1 because the original bootstrap
    # run happened before this line was added.)
    linux-modules-extra-$(uname -r)

# 2. Node.js 22 LTS via NodeSource. The setup script writes
#    the keyring to /etc/apt/keyrings/nodesource.gpg and the
#    repo list to /etc/apt/sources.list.d/nodesource.list.
if ! command -v node >/dev/null 2>&1 || [[ "$(node --version)" != v22.* ]]; then
    curl -fsSL https://deb.nodesource.com/setup_22.x | bash -
    apt-get install -y -qq nodejs
fi

# 3. PostgreSQL 16 via PGDG (apt.postgresql.org).
if ! command -v psql >/dev/null 2>&1; then
    install -d /usr/share/postgresql-common/pgdg
    curl -fsSL https://www.postgresql.org/media/keys/ACCC4CF8.asc \
        -o /usr/share/postgresql-common/pgdg/apt.postgresql.org.asc
    echo "deb [signed-by=/usr/share/postgresql-common/pgdg/apt.postgresql.org.asc] https://apt.postgresql.org/pub/repos/apt $(lsb_release -cs)-pgdg main" \
        > /etc/apt/sources.list.d/pgdg.list
    apt-get update -qq
    apt-get install -y -qq postgresql-16
fi

# 4. code-server (browser VS Code) via the official install script.
#    Bound to 127.0.0.1:8080 by default; expose via the Takeover
#    SSH tunnel pattern when needed.
if ! command -v code-server >/dev/null 2>&1; then
    curl -fsSL https://code-server.dev/install.sh | sh
    # code-server's systemd unit binds to 127.0.0.1:8080 by default;
    # leave it that way (Tyler's SSH tunnel pattern exposes it).
    systemctl enable --now code-server@bot 2>/dev/null || true
fi

# 5. dev user — mirrors the bot user (passwordless sudo, can
#    `su - dev` from bot). No password set; SSH-keyed access
#    only, same as bot.
if ! id -u dev >/dev/null 2>&1; then
    useradd -m -s /bin/bash -G sudo,adm dev
    passwd -d dev  # passwordless
    # Mirror bot's authorized_keys.
    mkdir -p /home/dev/.ssh
    chmod 700 /home/dev/.ssh
    cp /home/bot/.ssh/authorized_keys /home/dev/.ssh/authorized_keys
    chmod 600 /home/dev/.ssh/authorized_keys
    chown -R dev:dev /home/dev/.ssh
fi

# 6. v3.7.8 fix: bind the bochs-drm driver at boot
#    so LightDM + Xorg have a display on first start.
#    `linux-modules-extra-$(uname -r)` (installed in
#    step 1) provides the .ko; this line makes the
#    kernel actually load it. Without it, the next
#    boot will fall back to the same fbdev/vesa
#    failures that kept the VM at tty1.
if ! grep -q '^bochs$' /etc/modules-load.d/bochs.conf 2>/dev/null; then
    echo bochs > /etc/modules-load.d/bochs.conf
    modprobe bochs 2>/dev/null || true
fi

echo "---versions---"
node --version
npm --version
psql --version
code-server --version 2>/dev/null || true
echo "---users---"
id bot dev
echo "OK"
