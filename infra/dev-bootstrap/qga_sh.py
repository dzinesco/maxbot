#!/usr/bin/env python3
"""Run a command in a VM via QEMU guest agent and return stdout.

Usage: qga_sh.py <domain> <shell_cmd>
"""
import base64
import json
import subprocess
import sys
import time


def qga(domain: str, payload: dict) -> dict:
    r = subprocess.run(
        ["virsh", "qemu-agent-command", domain, json.dumps(payload)],
        capture_output=True,
        text=True,
        check=True,
    )
    return json.loads(r.stdout)["return"]


def main():
    domain = sys.argv[1]
    cmd = sys.argv[2]

    # Fire the exec.
    r = qga(domain, {
        "execute": "guest-exec",
        "arguments": {
            "path": "/bin/sh",
            "arg": ["-c", cmd],
            "capture-output": True,
        },
    })
    pid = r["pid"]

    # Wait for it to exit. Long timeout because apt installs +
    # Postgres / Node can take several minutes.
    for _ in range(300):
        s = qga(domain, {"execute": "guest-exec-status", "arguments": {"pid": pid}})
        if s.get("exited"):
            out = s.get("out-data", "")
            err = s.get("err-data", "")
            if out:
                sys.stdout.write(base64.b64decode(out).decode())
            if err:
                sys.stderr.write(base64.b64decode(err).decode())
            sys.exit(s.get("exitcode", 1))
        time.sleep(2)
    print("timed out waiting for exec to exit", file=sys.stderr)
    sys.exit(2)


if __name__ == "__main__":
    main()
