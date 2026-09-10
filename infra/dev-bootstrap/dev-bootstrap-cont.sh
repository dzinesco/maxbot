#!/usr/bin/env bash
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

# Node.js 22 LTS
if ! command -v node >/dev/null 2>&1; then
    curl -fsSL https://deb.nodesource.com/setup_22.x | bash -
    apt-get install -y -qq nodejs
fi

# PostgreSQL 16 via PGDG
if ! command -v psql >/dev/null 2>&1; then
    install -d /usr/share/postgresql-common/pgdg
    curl -fsSL https://www.postgresql.org/media/keys/ACCC4CF8.asc \
        -o /usr/share/postgresql-common/pgdg/apt.postgresql.org.asc
    echo "deb [signed-by=/usr/share/postgresql-common/pgdg/apt.postgresql.org.asc] https://apt.postgresql.org/pub/repos/apt $(lsb_release -cs)-pgdg main" \
        > /etc/apt/sources.list.d/pgdg.list
    apt-get update -qq
    apt-get install -y -qq postgresql-16
fi

# code-server
if ! command -v code-server >/dev/null 2>&1; then
    curl -fsSL https://code-server.dev/install.sh | sh
    systemctl enable --now code-server@bot 2>/dev/null || true
fi

# dev user
if ! id -u dev >/dev/null 2>&1; then
    useradd -m -s /bin/bash -G sudo,adm dev
    passwd -d dev
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
echo OK
