#!/usr/bin/env python3
"""Test the VNC handshake against the test VM to confirm no auth required."""
import socket
import sys

host = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
port = int(sys.argv[2]) if len(sys.argv) > 2 else 5950

s = socket.socket()
s.settimeout(3)
s.connect((host, port))
s.sendall(b"RFB 003.008\n")
data = b""
while b"\n" not in data or len(data) < 14:
    chunk = s.recv(64)
    if not chunk:
        break
    data += chunk
print(f"got: {data!r}")
lines = data.split(b"\n")
version = lines[0]
# After "RFB 003.008\n" (12 bytes), the server sends:
#   - 1 byte: number of security types
#   - N bytes: the security types
rest = data[len(version) + 1 :]  # skip version + newline
print(f"version: {version!r}")
print(f"security types: {[hex(b) for b in rest]}")
if rest and rest[0] == 1 and rest[1] == 1:
    print("server offers VNC_AUTH_NONE only -- no password prompt")
    s.sendall(b"\x01")
    result = s.recv(64)
    print(f"after sending type 1: {result!r}")
    if result == b"\x00\x00\x00\x00":
        print("HANDSHAKE COMPLETE — connection works, no password needed")
    else:
        print(f"unexpected result: {result!r}")
else:
    print(f"unexpected security negotiation: {rest!r}")
s.close()
