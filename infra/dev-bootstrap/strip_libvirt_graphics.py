#!/usr/bin/env python3
"""Strip the libvirt-managed <graphics> element from a domain XML,
leaving only the qemu:commandline VNC.

Usage: strip_libvirt_graphics.py <input.xml> <output.xml>
"""
import re
import sys

if len(sys.argv) != 3:
    print("usage: strip_libvirt_graphics.py <in.xml> <out.xml>", file=sys.stderr)
    sys.exit(2)

in_path, out_path = sys.argv[1], sys.argv[2]

with open(in_path) as f:
    x = f.read()

# Remove the entire <graphics>...</graphics> block. libvirt will not
# manage a VNC server anymore; the only VNC is from <qemu:commandline>.
x = re.sub(
    r"<graphics[^>]*>.*?</graphics>",
    "",
    x,
    flags=re.DOTALL,
)
# Also handle self-closing form <graphics .../>
x = re.sub(
    r"<graphics[^/]*/>",
    "",
    x,
)

with open(out_path, "w") as f:
    f.write(x)

print(f"wrote {out_path}", file=sys.stderr)
