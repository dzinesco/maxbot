#!/usr/bin/env python3
"""Patch a libvirt domain XML to inject a qemu:commandline that
disables VNC password auth.

Usage: patch_vnc_auth.py <input.xml> <output.xml>
"""
import re
import sys

if len(sys.argv) != 3:
    print("usage: patch_vnc_auth.py <in.xml> <out.xml>", file=sys.stderr)
    sys.exit(2)

in_path, out_path = sys.argv[1], sys.argv[2]

with open(in_path) as f:
    x = f.read()

# Strip any existing qemu:commandline block.
x = re.sub(r"<qemu:commandline>.*?</qemu:commandline>", "", x, flags=re.DOTALL)
# Strip any existing xmlns:qemu declaration.
x = re.sub(
    r"""\sxmlns:qemu=['"][^'"]+['"]""",
    "",
    x,
)

# Add the qemu namespace.
if "xmlns:qemu" not in x:
    x = x.replace(
        "<domain type=",
        "<domain xmlns:qemu='http://libvirt.org/schemas/domain/qemu/1.0' type=",
        1,
    )

# Add the qemu:commandline block with -vnc password=off,to=5999.
qemu_block = (
    "<qemu:commandline>"
    "<qemu:arg value='-vnc'/>"
    "<qemu:arg value='127.0.0.1:0,password=off,to=5999'/>"
    "</qemu:commandline>"
    "</domain>"
)
x = x.replace("</domain>", qemu_block, 1)

with open(out_path, "w") as f:
    f.write(x)

print(f"wrote {out_path}", file=sys.stderr)
