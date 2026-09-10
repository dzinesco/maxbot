#!/usr/bin/env bash
# Add Tyler's default SSH pubkey to a Bot's authorized_keys via QGA.
# Usage: qga_add_key.sh <domain> <pubkey_file>
set -euo pipefail

DOMAIN="$1"
PUBKEY_FILE="$2"
PUBKEY=$(cat "$PUBKEY_FILE")

# Single shell command: write key, set perms, chown to bot:bot.
SHELL_CMD="mkdir -p /home/bot/.ssh && chmod 700 /home/bot/.ssh && (grep -qxF '${PUBKEY}' /home/bot/.ssh/authorized_keys 2>/dev/null || printf '%s\n' '${PUBKEY}' >> /home/bot/.ssh/authorized_keys) && chmod 600 /home/bot/.ssh/authorized_keys && chown -R bot:bot /home/bot/.ssh && wc -l /home/bot/.ssh/authorized_keys"

# Build the QGA JSON payload via python to avoid escaping issues.
JSON=$(python3 -c "
import json, sys
cmd = sys.argv[1]
print(json.dumps({
    'execute': 'guest-exec',
    'arguments': {
        'path': '/bin/sh',
        'arg': ['-c', cmd],
        'capture-output': True,
    }
}))
" "$SHELL_CMD")

# Fire the exec. QGA returns a pid; we follow up with
# guest-exec-status to get stdout/stderr.
RESP=$(virsh qemu-agent-command "$DOMAIN" "$JSON" 2>&1)
echo "exec response: $RESP"
PID=$(echo "$RESP" | python3 -c "import sys, json; print(json.load(sys.stdin)['return']['pid'])")
sleep 1
STATUS=$(virsh qemu-agent-command "$DOMAIN" "$(python3 -c "
import json, sys
print(json.dumps({'execute': 'guest-exec-status', 'arguments': {'pid': int(sys.argv[1])}}))
" "$PID")")
echo "exec-status: $STATUS"
