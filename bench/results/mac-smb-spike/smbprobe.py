#!/usr/bin/python3
# SPDX-License-Identifier: Apache-2.0
"""A name this Mac looked up and missed, then created by another machine: how long until the mount
shows it, by path and in the folder's listing, for every hold of the FSKit blocker, a young or old
folder, and with or without a watcher on the folder.

    smbprobe.py <mountpoint> [--cases a,b] [--ages young,old] [--watch no,kqueue] [--limit S]
                [--age-cmd CMD] [--poll stat|list] -- <peer command…>

The peer is `mount_peer <drive>` or `localpeer.py <folder> <control>`. smbfs keeps a folder's
attributes for a tenth of its age (2 to 30 s, as NFS does) and drops its cached misses when it sees
the folder's time move; `--age-cmd CMD <key>` makes a folder old on the server (the local backend
sets its time back a day) so that the 30 s case is measured, not assumed. The top folder is as old as it is. A `kqueue` watcher is what
Finder windows and file watchers put on a folder, and what makes smbfs ask the server for change
notifications. Each case prints the milliseconds from the peer's commit until the name is found by
`stat` and until `readdir` lists it, while held and after the hold ends. The `unlisted` case looks the name up in
a folder this Mac has never listed (poll with `stat` for it).
"""

import os
import select
import subprocess
import sys
import time

argv = sys.argv[1:]
peer = argv[argv.index("--") + 1:]
opts = argv[:argv.index("--")]
mnt = opts[0]


def opt(name, default):
    return opts[opts.index(name) + 1] if name in opts else default


LIMIT = float(opt("--limit", "40"))
AGE_CMD = opt("--age-cmd", None)
CASES = opt("--cases", "nothing,sibling-file,subfolder-file,other-folder-file,cwd,dir-evtonly,top").split(",")
AGES = opt("--ages", "young,old").split(",")
WATCH = opt("--watch", "no,kqueue").split(",")
POLL = opt("--poll", "stat")
run = f"probe-{int(time.time())}-{os.getpid()}"


def remote(*args):
    out = subprocess.run(peer + [*args], check=True, capture_output=True, text=True).stdout
    return float(out.rsplit(" ", 1)[1])


def exists(p):
    try:
        os.stat(p)
        return True
    except FileNotFoundError:
        return False


def listed(d, name):
    try:
        return name in os.listdir(d)
    except FileNotFoundError:
        return False


def wait(test, limit, step=0.02):
    t = time.time()
    while time.time() - t < limit:
        try:
            if test():
                return time.time()
        except OSError:
            pass
        time.sleep(step)
    return None


def ms(at, since):
    return "never" if at is None else f"{(at - since) * 1000:.0f}"


def setup(key, old, unlisted=False):
    """A folder with a.txt and sub/x.txt, listed on the mount. An old folder is made old on the
    server before this Mac first looks at it: smbfs never lets a folder's time go backwards.
    An unlisted folder is only looked up, never listed: smbfs asks for a folder's lease only when
    it lists it."""
    for name in ("a.txt", "sub/x.txt"):
        remote("fill", f"{key}/{name}", "2", "65")
    if old:
        subprocess.run(AGE_CMD.split() + [key], check=True)
    d = os.path.join(mnt, key)
    if unlisted:
        if wait(lambda: exists(os.path.join(d, "a.txt")), 90) is None:
            sys.exit(f"setup: {key}/a.txt never found")
        age = time.time() - os.stat(d).st_mtime
        if old and age < 3600:
            sys.exit(f"setup: {key} is {age:.0f} s old on the mount")
        return age
    if wait(lambda: listed(d, "a.txt") and listed(os.path.join(d, "sub"), "x.txt"), 90) is None:
        sys.exit(f"setup: {key} never listed")
    os.listdir(d)
    os.stat(os.path.join(d, "sub", "x.txt"))
    age = time.time() - os.stat(d).st_mtime
    if old and age < 3600:
        sys.exit(f"setup: {key} is {age:.0f} s old on the mount")
    return age


def holder(kind, d):
    if kind == "sibling-file":
        f = open(os.path.join(d, "a.txt"), "rb")
        return lambda: f.close()
    if kind == "subfolder-file":
        f = open(os.path.join(d, "sub", "x.txt"), "rb")
        return lambda: f.close()
    if kind == "other-folder-file":
        f = open(os.path.join(mnt, run, "other", "y.txt"), "rb")
        return lambda: f.close()
    if kind == "cwd":
        p = subprocess.Popen(["/bin/sh", "-c", 'cd "$0" && exec sleep 600', d])
        time.sleep(0.2)
        return lambda: (p.terminate(), p.wait())
    if kind == "dir-evtonly":
        fd = os.open(d, os.O_RDONLY | 0x8000)
        return lambda: os.close(fd)
    return lambda: None


def watcher(kind, d):
    if kind != "kqueue":
        return lambda: None, lambda: 0
    fd = os.open(d, os.O_RDONLY | 0x8000)
    kq = select.kqueue()
    ev = select.kevent(fd, filter=select.KQ_FILTER_VNODE, flags=select.KQ_EV_ADD | select.KQ_EV_CLEAR,
                       fflags=select.KQ_NOTE_WRITE | select.KQ_NOTE_EXTEND | select.KQ_NOTE_ATTRIB | select.KQ_NOTE_LINK | select.KQ_NOTE_RENAME | select.KQ_NOTE_DELETE)
    kq.control([ev], 0, 0)
    seen = []

    def count():
        while True:
            got = kq.control(None, 8, 0)
            if not got:
                return len(seen)
            seen.extend(got)

    return lambda: (kq.close(), os.close(fd)), count


def case(kind, age, watch, n):
    top = kind == "top"
    key = run if top else f"{run}/{kind}-{age}-{watch}"
    aged = None
    if not top:
        aged = setup(key, age == "old", unlisted=kind == "unlisted")
    elif age == "old":
        aged = time.time() - os.stat(mnt).st_mtime
    d = os.path.join(mnt, key)
    folder = mnt if top else d
    name = f"new-{kind}-{age}-{watch}-{n}.txt" if not top else f"{run}-top-{age}-{watch}-{n}.txt"
    unwatch, events = watcher(watch, folder)
    release = holder(kind, d)
    p = os.path.join(folder, name)
    missing = not exists(p)
    if kind != "unlisted":
        os.listdir(folder)
    at = remote("fill", (key + "/" + name) if not top else name, "5", "110")
    if POLL == "stat":
        found = wait(lambda: exists(p), LIMIT)
    else:
        found = wait(lambda: listed(folder, name), LIMIT)
    released = time.time()
    release()
    after = wait(lambda: exists(p) if POLL == "stat" else listed(folder, name), LIMIT) if found is None else None
    ev = events()
    unwatch()
    what = "found by path" if POLL == "stat" else "listed"
    print(f"{kind:18} {age:5} watch={watch:6} missed={missing} {what} {ms(found, at):>6} ms"
          + (f"; after release {ms(after, released)} ms" if found is None else "")
          + (f"; kqueue events {ev}" if watch == "kqueue" else "")
          + (f"; folder {aged:.0f} s old" if aged is not None else ""), flush=True)


def main():
    remote("fill", f"{run}/other/y.txt", "2", "89")
    wait(lambda: listed(os.path.join(mnt, run, "other"), "y.txt"), 90)
    n = 0
    for age in AGES:
        for watch in WATCH:
            for kind in CASES:
                n += 1
                case(kind, age, watch, n)


main()
