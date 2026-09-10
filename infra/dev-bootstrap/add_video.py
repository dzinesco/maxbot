#!/usr/bin/env python3
"""Replace the bad -vga std qemu:commandline args with a proper
<video><model type='vga'/></video> libvirt element. Idempotent.

Why: putting -vga std in qemu:commandline collides with libvirt's
pcie-root-port slot allocation ("PCI: slot 1 function 0 not
available for pcie-root-port, in use by VGA"). Using libvirt's
<video> element instead lets libvirt allocate the right PCI slot.
"""
import re
import sys

if len(sys.argv) != 3:
    print("usage: add_video.py <in.xml> <out.xml>", file=sys.stderr)
    sys.exit(2)

in_path, out_path = sys.argv[1], sys.argv[2]

with open(in_path) as f:
    x = f.read()

# Strip the -vga std qemu:arg pair from the qemu:commandline block.
# Pair is the next two <qemu:arg> elements after <qemu:arg value='-vga'/>.
m = re.search(r"(<qemu:commandline>)(.*?)(</qemu:commandline>)", x, flags=re.DOTALL)
if m and "<qemu:arg value='-vga'/>" in m.group(2):
    inner = m.group(2)
    inner = re.sub(
        r"<qemu:arg value='-vga'/>\s*<qemu:arg value='std'/>\s*",
        "",
        inner,
    )
    new_block = m.group(1) + inner + m.group(3)
    x = x[: m.start()] + new_block + x[m.end() :]

# Strip any existing <video>...</video> block.
x = re.sub(r"<video>.*?</video>", "", x, flags=re.DOTALL)
x = re.sub(r"<video[^/]*/>", "", x)

# Add <video><model type='vga'/></video> right before </devices>.
if "<video>" not in x:
    x = x.replace("</devices>", "<video><model type='vga'/></video></devices>", 1)

with open(out_path, "w") as f:
    f.write(x)

print(f"wrote {out_path}", file=sys.stderr)
