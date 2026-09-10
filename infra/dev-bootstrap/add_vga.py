#!/usr/bin/env python3
"""Add -vga std to the existing <qemu:commandline> block in a
libvirt domain XML. Idempotent (skips if already present)."""
import re
import sys

if len(sys.argv) != 3:
    print("usage: add_vga.py <in.xml> <out.xml>", file=sys.stderr)
    sys.exit(2)

in_path, out_path = sys.argv[1], sys.argv[2]

with open(in_path) as f:
    x = f.read()

# Find the existing qemu:commandline block.
m = re.search(r"(<qemu:commandline>)(.*?)(</qemu:commandline>)", x, flags=re.DOTALL)
if not m:
    print("no qemu:commandline block found", file=sys.stderr)
    sys.exit(3)

inner = m.group(2)

# Skip if -vga already present.
if "value='-vga'" in inner or 'value="-vga"' in inner:
    print("already has -vga; skipping", file=sys.stderr)
    with open(out_path, "w") as f:
        f.write(x)
    sys.exit(0)

# Inject -vga std as the first pair in the block.
new_inner = (
    "<qemu:arg value='-vga'/>"
    "<qemu:arg value='std'/>"
    + inner
)
new_block = m.group(1) + new_inner + m.group(3)
x = x[: m.start()] + new_block + x[m.end() :]

with open(out_path, "w") as f:
    f.write(x)

print(f"wrote {out_path} (added -vga std)", file=sys.stderr)
