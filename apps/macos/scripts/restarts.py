#!/usr/bin/python3
# SPDX-License-Identifier: Apache-2.0
"""A mounted drive across a restart of the daemon or the launch agent: step 5, item 3.

    /usr/bin/python3 apps/macos/scripts/restarts.py <mountpoint> <label> <command…>

Opens many/f0042.txt (from the drive `mount_seed` fills) with F_NOCACHE and reads it, runs the
command (which restarts or kills the daemon or the agent), then polls every 100 ms until the open
file reads the same bytes again and a stat and a listing work. Exits 1 if that takes over 60 s.
"""

import fcntl
import os
import subprocess
import sys
import time

mnt, label, cmd = sys.argv[1], sys.argv[2], sys.argv[3:]
p = os.path.join(mnt, "many/f0042.txt")
fd = os.open(p, os.O_RDONLY)
fcntl.fcntl(fd, 48, 1)
before = os.pread(fd, 100, 0)
subprocess.run(cmd, check=False, capture_output=True)
start, last, ok = time.time(), None, False
while time.time() - start < 60:
    try:
        ok = os.pread(fd, 100, 0) == before and os.stat(os.path.join(mnt, "hello.txt")).st_size == 18 and "f0999.txt" in os.listdir(os.path.join(mnt, "many"))
        if ok:
            break
    except OSError as e:
        last = e
    time.sleep(0.1)
elapsed = (time.time() - start) * 1000
meanwhile = f" (meanwhile: {last})" if last else ""
print(f"{'ok  ' if ok else 'FAIL'} {label}: the open file reads the same bytes, and a stat and a listing work, {'after' if ok else 'not within'} {elapsed:.0f} ms{meanwhile}", flush=True)
os.close(fd)
sys.exit(0 if ok else 1)
