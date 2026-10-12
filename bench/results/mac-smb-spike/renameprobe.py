#!/usr/bin/python3
# SPDX-License-Identifier: Apache-2.0
"""Another machine renames a file this Mac has seen: how long until the new name is listed and
opens, and the old name is gone, when this Mac had only listed the file, had read it, or still
has it open.

    renameprobe.py <mountpoint> [--limit S] -- <peer command…>

smbfs knows files by the server's file ID; a file it still has a node for that turns up under a
new name with the same ID may be opened by its old name.
"""

import os
import subprocess
import sys
import time

argv = sys.argv[1:]
peer = argv[argv.index("--") + 1:]
opts = argv[:argv.index("--")]
mnt = opts[0]
LIMIT = float(opts[opts.index("--limit") + 1]) if "--limit" in opts else 30.0
run = f"rename-{int(time.time())}-{os.getpid()}"


def remote(*args):
    out = subprocess.run(peer + [*args], check=True, capture_output=True, text=True).stdout
    return float(out.rsplit(" ", 1)[1])


def ms(t):
    return "never" if t is None else f"{t * 1000:.0f} ms"


def opens(p, data):
    try:
        with open(p, "rb") as f:
            return f.read() == data
    except OSError:
        return False


def case(how):
    src, dst = f"{run}/{how}.txt", f"{run}/{how}-to.txt"
    data = how.encode() * 3
    remote("put", src, data.decode())
    folder, p, q = os.path.join(mnt, run), os.path.join(mnt, src), os.path.join(mnt, dst)
    t0 = time.time()
    while not (os.path.isdir(folder) and os.path.basename(p) in os.listdir(folder)):
        if time.time() - t0 > 30:
            sys.exit(f"{src} never listed")
        time.sleep(0.05)
    held = None
    if how in ("read", "open"):
        with open(p, "rb") as f:
            f.read()
    if how == "open":
        held = open(p, "rb")
    at = remote("rename", src, dst)
    seen = {"listed": None, "opens": None, "old gone": None}
    while time.time() - at < LIMIT and None in seen.values():
        now = time.time() - at
        if seen["listed"] is None and os.path.basename(q) in os.listdir(folder):
            seen["listed"] = now
        if seen["opens"] is None and opens(q, data):
            seen["opens"] = now
        if seen["old gone"] is None and not os.path.exists(p):
            seen["old gone"] = now
        time.sleep(0.01)
    still = held.read() == data if held else None
    if held:
        held.close()
    print(f"{how:7} new name listed {ms(seen['listed']):>8}, opens {ms(seen['opens']):>8}, old name gone {ms(seen['old gone']):>8}"
          + (f"; the open file still reads it: {still}" if held else ""), flush=True)


for how in ("listed", "read", "open"):
    case(how)
