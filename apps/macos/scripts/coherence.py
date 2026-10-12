#!/usr/bin/python3
# SPDX-License-Identifier: Apache-2.0
"""Another machine's changes, seen through a mounted drive: the coherence cases of step 5, item 3.

    /usr/bin/python3 apps/macos/scripts/coherence.py <mountpoint> <peer command…>

The peer is a second client of the same server, such as the SDK's `mount_peer` example
(`target/release/examples/mount_peer <drive>`, with `VOIDFS_ENDPOINT`, `VOIDFS_ACCESS_KEY_ID` and
`VOIDFS_SECRET_ACCESS_KEY` set). In a fresh folder it overwrites (growing and shrinking), renames,
deletes and creates files while this script holds a sibling open there; the mount is polled every
10 ms from the moment the peer's commit returned. Each case then runs again with the changed file
itself open on the mount, read with F_NOCACHE so every read reaches the file system: the open file
must keep the version it opened, whole, never an old length with new bytes.

FSKit's limits (step 5, "Read-only FSKit on the daemon") are reported as `limit` lines with what
was seen, and fail only if the mount does worse than them: an item in use (open here, or below a
folder in use) keeps its cached name and attributes until it is let go, so its path shows the
version it has open; a name looked up and missed stays missed while anything below its folder is
in use, and in the drive's top folder until the drive is mounted again. New names are watched for
in the folder's listing first, since `readdir` never makes the kernel cache a miss.

Exits 1 if a check fails.
"""

import fcntl
import os
import subprocess
import sys
import time

F_NOCACHE = 48
LIMIT = 5.0  # seconds: step 5's requirement for a change to show

mnt, peer = sys.argv[1], sys.argv[2:]
run = f"coherence-{int(time.time())}-{os.getpid()}"
folder = os.path.join(mnt, run)
failures = 0


def remote(*args):
    out = subprocess.run(peer + [*args], check=True, capture_output=True, text=True).stdout
    return float(out.rsplit(" ", 1)[1])


def put(name, data):
    return remote("fill", f"{run}/{name}", str(len(data)), str(data[0]))


def check(name, ok, detail=""):
    global failures
    if not ok:
        failures += 1
    print(f"{'ok  ' if ok else 'FAIL'} {name}{'' if ok else ': ' + detail}", flush=True)


def limit(text):
    print(f"limit {text}", flush=True)


def watch(since, tests, limit=LIMIT):
    """For each named test, the seconds from `since` until it first held, or None."""
    seen = {name: None for name in tests}
    while time.time() - since < limit and None in seen.values():
        for name, test in tests.items():
            if seen[name] is None:
                try:
                    if test():
                        seen[name] = time.time() - since
                except OSError:
                    pass
        time.sleep(0.01)
    return seen


def exists(path):
    try:
        os.stat(path)
        return True
    except FileNotFoundError:
        return False


def listed(directory, name):
    return name in os.listdir(directory)


def contents(path):
    with open(path, "rb") as f:
        return f.read()


def open_nocache(path):
    fd = os.open(path, os.O_RDONLY)
    fcntl.fcntl(fd, F_NOCACHE, 1)
    return fd


def read_all(fd):
    """The descriptor's bytes, or the error reading them stopped at (a failed check, not a crash)."""
    out, offset = b"", 0
    while True:
        try:
            piece = os.pread(fd, 1 << 20, offset)
        except OSError as e:
            return f"{e} at {offset}"
        if not piece:
            return out
        out, offset = out + piece, offset + len(piece)


def ms(seconds):
    return "never" if seconds is None else f"{seconds * 1000:.0f} ms"


def path(name):
    return os.path.join(folder, name)


def overwrite(name, old, new):
    p = path(name)
    at = put(name, new)
    t = watch(at, {"path": lambda: os.stat(p).st_size == len(new) and contents(p) == new})["path"]
    check(f"{name}: an overwrite {len(old)} → {len(new)} bytes, a sibling held open, shows by path within {LIMIT:.0f} s ({ms(t)})", t is not None)


def uncached(p):
    fd = open_nocache(p)
    try:
        return read_all(fd)
    finally:
        os.close(fd)


def overwrite_open(name, old, new):
    # Only F_NOCACHE reads, and only the first page before the change: the kernel caches none of
    # it, so what the open file reads afterwards comes from the file system's handle.
    p = path(name)
    put(name, old)
    if watch(time.time(), {"path": lambda: os.stat(p).st_size == len(old) and uncached(p) == old})["path"] is None:
        check(f"{name}: back to {len(old)} bytes", False)
        return
    fd = open_nocache(p)
    first = os.pread(fd, 4096, 0)
    at = put(name, new)
    t = watch(at, {"path": lambda: os.stat(p).st_size == len(new)}, 2.0)["path"]
    after, size = read_all(fd), os.fstat(fd).st_size
    check(f"{name}: open while overwritten {len(old)} → {len(new)} bytes, it keeps the version it opened, whole ({len(after)} bytes, fstat {size})",
          first == old[:4096] and after == old and size == len(old), f"read {after[:40]!r}… ({len(after)} bytes), fstat {size}")
    if t is None:
        limit(f"{name}: while it is open here, its path shows the version open ({os.stat(p).st_size} bytes)")
    closed = time.time()
    os.close(fd)
    t = watch(closed, {"path": lambda: os.stat(p).st_size == len(new) and uncached(p) == new})["path"]
    check(f"{name}: once closed, its path shows the new version ({ms(t)})", t is not None)


def rename(src, dst, hold):
    p, q = path(src), path(dst)
    data = contents(p)
    fd = open_nocache(p) if hold else None
    at = remote("rename", f"{run}/{src}", f"{run}/{dst}")
    # The new name opening is watched for too: a mount can list it before it opens.
    t = watch(at, {"gone": lambda: not exists(p), "listed": lambda: listed(folder, dst), "opens": lambda: contents(q) == data},
              2.0 if hold else LIMIT)
    opens = t["listed"] is not None and t["opens"] is not None
    if not hold:
        check(f"{src} → {dst}: a rename, a sibling held open, shows within {LIMIT:.0f} s: old name gone ({ms(t['gone'])}), new name listed ({ms(t['listed'])}) and opens ({ms(t['opens'])})", t["gone"] is not None and opens)
        return
    check(f"{src} → {dst}: open while renamed, it still reads it; the new name is listed ({ms(t['listed'])}) and opens ({ms(t['opens'])})", read_all(fd) == data and opens)
    if t["gone"] is None:
        limit(f"{src}: while it is open here, its old name still finds it")
    closed = time.time()
    os.close(fd)
    t = watch(closed, {"gone": lambda: not exists(p)})["gone"]
    check(f"{src}: once closed, the old name is gone ({ms(t)})", t is not None)


def delete(name, hold):
    p = path(name)
    data = contents(p)
    fd = open_nocache(p) if hold else None
    at = remote("delete", f"{run}/{name}")
    t = watch(at, {"gone": lambda: not exists(p), "unlisted": lambda: not listed(folder, name)}, 2.0 if hold else LIMIT)
    if not hold:
        check(f"{name}: a delete, a sibling held open, shows within {LIMIT:.0f} s: by path ({ms(t['gone'])}), listing ({ms(t['unlisted'])})", t["gone"] is not None and t["unlisted"] is not None)
        return
    check(f"{name}: open while deleted, it still reads it; the listing drops it ({ms(t['unlisted'])})", read_all(fd) == data and t["unlisted"] is not None)
    if t["gone"] is None:
        limit(f"{name}: while it is open here, its name still finds it")
    closed = time.time()
    os.close(fd)
    t = watch(closed, {"gone": lambda: not exists(p)})["gone"]
    check(f"{name}: once closed, it is gone ({ms(t)})", t is not None)


def created(name):
    at = put(name, b"n" * 5)
    t = watch(at, {"listed": lambda: listed(folder, name)})["listed"]
    check(f"{name}: a new file, a sibling held open, is listed within {LIMIT:.0f} s ({ms(t)}) and opens", t is not None and contents(path(name)) == b"n" * 5)


def ls(directory):
    return subprocess.run(["/bin/ls", directory], capture_output=True, text=True).stdout.split("\n")


def missed_then_created(directory, name, held, where):
    p = os.path.join(directory, name)
    key = os.path.relpath(p, mnt)
    check(f"{key}: missing before it is created", not exists(p))
    # Listed, twice, as a Finder window or a shell would, before it is created.
    ls(directory)
    ls(directory)
    at = remote("fill", key, "5", "110")
    t = watch(at, {"path": lambda: exists(p), "listed": lambda: name in ls(directory)}, 3.0)
    check(f"{key}: looked up and missed, then created, {where}: listed within {LIMIT:.0f} s ({ms(t['listed'])})", t["listed"] is not None)
    if held is None and directory != mnt:
        check(f"{key}: looked up and missed, then created, {where}: found by path within {LIMIT:.0f} s ({ms(t['path'])})", t["path"] is not None)
    elif t["path"] is None:
        limit(f"{key}: looked up and missed, then created, {where}: not found by path within 3 s")
    else:
        print(f"note {key}: looked up and missed, then created, {where}: found by path in {ms(t['path'])}", flush=True)
    if held is not None:
        # Files looked up meanwhile are cached again below the folder, and would hold its purge
        # past the sibling's close if the purge weren't tried again then.
        for n in ("grows.bin", "shrinks.bin", "renamed-to.txt", "created.txt"):
            exists(path(n))
        released = time.time()
        held.close()
        t = watch(released, {"path": lambda: exists(p)})["path"]
        check(f"{key}: found by path once the sibling closes ({ms(t)})", t is not None)


def missed_folder_then_created():
    """A folder looked up and missed, then created with a file in it: the miss is on a name
    along the new key, not its last one."""
    p = path("later/inside.txt")
    check(f"{run}/later: missing before it is created", not exists(path("later")) and not exists(p))
    at = remote("fill", f"{run}/later/inside.txt", "5", "110")
    t = watch(at, {"path": lambda: exists(p)})["path"]
    check(f"{run}/later/inside.txt: a folder looked up and missed, then created with a file in it, nothing held: found by path within {LIMIT:.0f} s ({ms(t)})", t is not None)


def main():
    # Everything the cases change exists first, and is found by listing. A subfolder is visited,
    # so purging a folder has more than its own files to go through.
    files = {"held.txt": b"H" * 12, "grows.bin": b"A" * 12, "shrinks.bin": b"C" * (3 << 20), "renamed.txt": b"R" * 10,
             "renamed-open.txt": b"S" * 10, "deleted.txt": b"Z" * 10, "deleted-open.txt": b"Y" * 10, "sub/inside.txt": b"I" * 10}
    for name, data in files.items():
        at = put(name, data)
    t = watch(at, {"listed": lambda: listed(mnt, run) and all(listed(os.path.dirname(path(n)), os.path.basename(n)) for n in files)})["listed"]
    check(f"a new folder of {len(files)} files is listed within {LIMIT:.0f} s ({ms(t)})", t is not None)
    if t is None:
        sys.exit(1)
    check("and each opens by path with its bytes", all(contents(path(n)) == d for n, d in files.items()))
    held = open(path("held.txt"), "rb")
    print(f"holding {run}/held.txt open", flush=True)

    overwrite("grows.bin", files["grows.bin"], b"B" * 32)
    overwrite("shrinks.bin", files["shrinks.bin"], b"D" * 8)
    overwrite_open("grows.bin", b"E" * 12, b"F" * 32)
    overwrite_open("shrinks.bin", b"G" * (3 << 20), b"I" * 8)
    rename("renamed.txt", "renamed-to.txt", False)
    rename("renamed-open.txt", "renamed-open-to.txt", True)
    delete("deleted.txt", False)
    delete("deleted-open.txt", True)
    created("created.txt")

    missed_then_created(folder, "missed-while-held.txt", held, "a sibling held open")
    missed_then_created(folder, "missed-nothing-held.txt", None, "nothing held")
    missed_folder_then_created()
    missed_then_created(mnt, f"{run}-top.txt", None, "in the drive's top folder")

    print(f"{failures} failed", flush=True)
    sys.exit(1 if failures else 0)


main()
