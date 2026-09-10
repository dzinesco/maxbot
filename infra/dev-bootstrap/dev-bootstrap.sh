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
    fd-find

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

echo "---versions---"
node --version
npm --version
psql --version
code-server --version 2>/dev/null || true
echo "---users---"
id bot dev
echo "OK"
