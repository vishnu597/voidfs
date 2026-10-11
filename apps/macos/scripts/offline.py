#!/usr/bin/python3
# SPDX-License-Identifier: Apache-2.0
"""A mounted drive while its server can't be reached, and after: step 5, item 3's offline bullet.

    /usr/bin/python3 apps/macos/scripts/offline.py <mountpoint> <relay pid> <relay command> <peer command…>

The daemon must reach the server through a relay this script controls (any TCP forwarder; the
item 3 runs used a small Python one): killing it makes the server refuse connections, stopping it
with SIGSTOP makes it hang. The peer (`mount_peer <drive>`) reaches the server directly, as another
machine would, and changes the drive while this Mac is cut off. Uses the drive `mount_seed` fills.

Cached metadata and bytes must be served offline; uncached ones must fail within the bound
below; once the relay is back, the change made meanwhile must show, which needs the feed resumed
and the mount's state refreshed. Each operation runs in its own process, so a hung one can't hang
this script. Exits 1 if a check fails.
"""

import os
import signal
import subprocess
import sys
import time

PROMPT = 10.0  # seconds within which an uncached call must fail
LIMIT = 5.0

mnt, relay_pid, relay_cmd = sys.argv[1], int(sys.argv[2]), sys.argv[3]
peer = sys.argv[4:]
failures = 0
tag = f"{int(time.time())}-{os.getpid()}"


def check(name, ok, detail=""):
    global failures
    if not ok:
        failures += 1
    print(f"{'ok  ' if ok else 'FAIL'} {name}{'' if ok else ': ' + detail}", flush=True)


def timed(*cmd, limit=60):
    """(seconds, exit status, output) of a command on the mount; status None if it took over `limit`."""
    start = time.time()
    try:
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=limit)
        return time.time() - start, p.returncode, p.stdout + p.stderr
    except subprocess.TimeoutExpired:
        return time.time() - start, None, "timed out"


def nocache_read(path):
    # Every read reaches the file system: the kernel's own cache can't answer for the daemon.
    return ("/usr/bin/python3", "-I", "-c", "import fcntl,os,sys; fd=os.open(sys.argv[1],os.O_RDONLY); fcntl.fcntl(fd,48,1); sys.stdout.write(os.read(fd,1<<20).decode())", path)


def ms(s):
    return f"{s * 1000:.0f} ms"


def offline(kind, unread, unlisted, unread_bytes, unlisted_entry):
    global relay_pid
    m = lambda p: os.path.join(mnt, p)
    # Warm: one folder listed, ten files read, one file looked up but not read.
    for cmd in [("ls", m("many")), ("cat", *[m(f"many/f000{i}.txt") for i in range(10)]), ("stat", m("docs/nested/deep/readme.md"))]:
        assert timed(*cmd)[1] == 0, cmd
    if kind == "refused":
        os.kill(relay_pid, signal.SIGTERM)
    else:
        os.kill(relay_pid, signal.SIGSTOP)
    print(f"== the server {kind}", flush=True)
    t, status, out = timed("ls", m("many"))
    check(f"{kind}: a listed folder is listed ({ms(t)})", status == 0 and "f0999.txt" in out, out[:200])
    t, status, out = timed(*nocache_read(m("many/f0003.txt")))
    check(f"{kind}: a file read before reads, from the daemon's cache ({ms(t)})", status == 0 and out == "file 3 of 1000\n", out[:200])
    t, status, out = timed("stat", m("docs/nested/deep/readme.md"))
    check(f"{kind}: a file looked up before stats ({ms(t)})", status == 0, out[:200])
    t, status, out = timed(*nocache_read(m(unread)))
    check(f"{kind}: bytes never read ({unread}) fail within {PROMPT:.0f} s ({ms(t)}: {out.strip()[-80:]})", status not in (0, None) and t < PROMPT, out[:200])
    t, status, out = timed(*nocache_read(m(unread)))
    check(f"{kind}: and again ({ms(t)})", status not in (0, None) and t < PROMPT, out[:200])
    t, status, out = timed("ls", m(unlisted))
    check(f"{kind}: a folder never listed ({unlisted}) fails within {PROMPT:.0f} s ({ms(t)}: {out.strip()[-80:]})", status not in (0, None) and t < PROMPT, out[:200])
    changed = f"offline-{kind}-{tag}.txt"
    subprocess.run(peer + ["put", f"probe/{changed}", "made while this Mac was cut off"], check=True, capture_output=True)
    # Back.
    if kind == "refused":
        relay_pid = subprocess.Popen(relay_cmd.split(), stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True).pid
    else:
        os.kill(relay_pid, signal.SIGCONT)
    back = time.time()
    print(f"== the server back", flush=True)
    seen = None
    while time.time() - back < 60:
        if timed("ls", m("probe"))[2].split().count(changed):
            seen = time.time() - back
            break
        time.sleep(0.2)
    check(f"{kind}: the change made meanwhile shows once the server is back ({'never' if seen is None else ms(seen)})", seen is not None)
    t, status, out = timed(*nocache_read(m(unread)))
    check(f"{kind}: the bytes never read now read ({ms(t)})", status == 0 and out == unread_bytes, out[:200])
    t, status, out = timed("ls", m(unlisted))
    check(f"{kind}: the folder never listed now lists ({ms(t)})", status == 0 and unlisted_entry in out.split(), out[-200:])


offline("refused", "docs/nested/deep/readme.md", "many10k", "# deep\n", "f09999.txt")
offline("hung", "many/f0500.txt", "media", "file 500 of 1000\n", "big.bin")
print(f"{failures} failed", flush=True)
sys.exit(1 if failures else 0)
