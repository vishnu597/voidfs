#!/usr/bin/python3
# SPDX-License-Identifier: Apache-2.0
"""Read a new file's extended attributes back at once, in Finder's copy order.

/usr/bin/python3 xattr-after-write.py <folder on a mount> [--finder] [--rounds N] [--settle SECONDS] [--keep]

Finder copies a file by creating it, writing its bytes, then setting its attributes. Each round
makes such a file in a fresh child of <folder>, then straight after, as a reader would, opens it
read-only, lists and reads its attributes on that descriptor, closes it, and lists them by path.
The attributes are a 26-byte user.voidfs.sentinel and a 4,096-byte com.apple.ResourceFork.

- By default the script makes the file itself: create and close it, remove the
  com.apple.provenance the kernel tags a tracked process's files with, open it for writing,
  write 43,008 bytes, set the attributes on that descriptor, and close it. A round whose file
  held an attribute before its write (the tag stayed, or came back) is inconclusive: the bug
  needs a file with none when it's first written. On 15 October, run from a tracked process,
  the tag stayed on APFS, and on the FSKit mount the kernel set it again 7 ms after its removal,
  so every such round was inconclusive.
- --finder makes the source on APFS (in a temporary folder) and has Finder copy it into the
  child with `duplicate`, as bench/results/fskit-large-xattrs did. It needs osascript's
  Automation access to Finder. This is the mode that reproduces the bug.

With --settle, a round that misses some opens and lists again every half second, for at most that
long, and records when they appeared.

Before the fix, the FSKit volume's flistxattr on the reader's descriptor listed none until the
file's staged bytes published: 3 of 3 Finder rounds, which listed them 1.1 s after the copy
(bench/results/fskit-xattrs-after-setxattr). `cp` didn't show it, as the kernel tagged its files
before their first open. One JSON line a round. Exit 0 when every round read every value back whole at once and at least one round was
conclusive, 1 when a round missed one, 2 on a setup failure or no conclusive round. It never
mounts, registers or starts anything.
"""

import argparse
import ctypes
import errno
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

DATA_SIZE = 43008
VALUES = {'user.voidfs.sentinel': 26, 'com.apple.ResourceFork': 4096}
PROVENANCE = 'com.apple.provenance'
FINDER_COPY = ['on run argv',
               'set sourceFile to (POSIX file (item 1 of argv)) as alias',
               'set targetFolder to (POSIX file (item 2 of argv)) as alias',
               'tell application "Finder" to set made to duplicate sourceFile to targetFolder',
               'return POSIX path of (made as alias)',
               'end run']

libc = ctypes.CDLL('/usr/lib/libSystem.B.dylib', use_errno=True)
libc.listxattr.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_size_t, ctypes.c_int]
libc.listxattr.restype = ctypes.c_ssize_t
libc.flistxattr.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_size_t, ctypes.c_int]
libc.flistxattr.restype = ctypes.c_ssize_t
libc.fgetxattr.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_uint32, ctypes.c_int]
libc.fgetxattr.restype = ctypes.c_ssize_t
libc.fsetxattr.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_uint32, ctypes.c_int]
libc.fsetxattr.restype = ctypes.c_int
libc.removexattr.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_int]
libc.removexattr.restype = ctypes.c_int


def fail(what):
    e = ctypes.get_errno()
    raise OSError(e, f'{what}: {os.strerror(e)}')


def names(listed):
    size = listed(None, 0)
    if size < 0:
        fail('list size')
    buf = ctypes.create_string_buffer(max(size, 1))
    size = listed(buf, size)
    if size < 0:
        fail('list')
    return sorted(n.decode() for n in buf.raw[:size].split(b'\0') if n)


def fd_names(fd):
    return names(lambda buf, size: libc.flistxattr(fd, buf, size, 0))


def path_names(path):
    p = os.fsencode(path)
    return names(lambda buf, size: libc.listxattr(p, buf, size, 0))


def fd_value(fd, name):
    n = name.encode()
    size = libc.fgetxattr(fd, n, None, 0, 0, 0)
    if size < 0:
        return None, ctypes.get_errno()
    buf = ctypes.create_string_buffer(max(size, 1))
    size = libc.fgetxattr(fd, n, buf, size, 0, 0)
    if size < 0:
        return None, ctypes.get_errno()
    return buf.raw[:size], 0


def value(name, size, seed):
    out = bytearray()
    counter = 0
    while len(out) < size:
        out += hashlib.sha256(f'{name} {seed} {counter}'.encode()).digest()
        counter += 1
    return bytes(out[:size])


def write_file(path, wanted, seed, flags):
    fd = os.open(path, flags, 0o644)
    try:
        os.write(fd, value('data', DATA_SIZE, seed))
        for name, v in wanted.items():
            buf = ctypes.create_string_buffer(v, len(v))
            if libc.fsetxattr(fd, name.encode(), buf, len(v), 0, 0) != 0:
                fail(f'fsetxattr {name}')
    finally:
        os.close(fd)


def by_syscalls(folder, index, wanted, record):
    path = os.path.join(folder, f'copied-{index}.bin')
    os.close(os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o644))
    record['taggedAtCreate'] = path_names(path)
    for name in record['taggedAtCreate']:
        libc.removexattr(os.fsencode(path), name.encode(), 0)
    kept = path_names(path)
    write_file(path, wanted, index, os.O_WRONLY)
    if kept:
        record['inconclusive'] = f'the file kept {kept} before its write'
    elif PROVENANCE in record['taggedAtCreate'] and PROVENANCE in path_names(path):
        record['inconclusive'] = f'the kernel tagged {PROVENANCE} again before or at the write'
    return path


def by_finder(folder, index, wanted, record, sources):
    source = os.path.join(sources, f'copied-{index}.bin')
    write_file(source, wanted, index, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
    p = subprocess.run(['/usr/bin/osascript'] + [a for line in FINDER_COPY for a in ('-e', line)] + [source, folder],
                       capture_output=True, text=True, timeout=120)
    if p.returncode != 0:
        raise OSError(errno.EIO, f'Finder copy: {p.stderr.strip()}')
    record['sourceListed'] = path_names(source)
    return p.stdout.strip() or os.path.join(folder, os.path.basename(source))


def read_back(path, wanted):
    """Opens `path` read-only and reads every attribute on that descriptor, as a reader would."""
    fd = os.open(path, os.O_RDONLY)
    try:
        while os.read(fd, 1 << 20):
            pass
        listed = fd_names(fd)
        whole = {}
        for name, want in wanted.items():
            got, e = fd_value(fd, name)
            whole[name] = 'whole' if got == want else errno.errorcode.get(e, str(e)) if got is None else f'{len(got)} bytes, different'
    finally:
        os.close(fd)
    return listed, whole


def round_trip(folder, index, settle, sources):
    record = {'round': index, 'how': 'finder' if sources else 'syscalls'}
    wanted = {name: value(name, size, index) for name, size in VALUES.items()}
    path = by_finder(folder, index, wanted, record, sources) if sources else by_syscalls(folder, index, wanted, record)
    made = time.monotonic()
    record['path'] = path
    listed, whole = read_back(path, wanted)
    record['readerListed'], record['readerValues'] = listed, whole
    record['pathListed'] = path_names(path)
    ok = lambda listed, whole: set(wanted) <= set(listed) and all(v == 'whole' for v in whole.values())
    record['atOnce'] = ok(listed, whole) and set(wanted) <= set(record['pathListed'])
    if not record['atOnce'] and settle > 0:
        record['appearedAfterSeconds'] = None
        while time.monotonic() - made < settle:
            time.sleep(0.5)
            if ok(*read_back(path, wanted)):
                record['appearedAfterSeconds'] = round(time.monotonic() - made, 1)
                break
    return record


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('folder')
    ap.add_argument('--finder', action='store_true')
    ap.add_argument('--rounds', type=int, default=5)
    ap.add_argument('--settle', type=float, default=0)
    ap.add_argument('--keep', action='store_true')
    a = ap.parse_args()
    try:
        folder = os.path.join(a.folder, f'xattr-after-write-{os.getpid()}-{int(time.time())}')
        os.mkdir(folder)
        sources = tempfile.mkdtemp(prefix='xattr-after-write-') if a.finder else None
    except OSError as e:
        print(json.dumps({'setup': str(e)}), flush=True)
        return 2
    missed = conclusive = 0
    try:
        for i in range(a.rounds):
            try:
                record = round_trip(folder, i, a.settle, sources)
            except (OSError, subprocess.TimeoutExpired) as e:
                print(json.dumps({'round': i, 'setup': str(e)}), flush=True)
                return 2
            print(json.dumps(record, sort_keys=True), flush=True)
            missed += not record['atOnce']
            conclusive += 'inconclusive' not in record
    finally:
        if not a.keep:
            shutil.rmtree(folder, ignore_errors=True)
        if sources:
            shutil.rmtree(sources, ignore_errors=True)
    print(json.dumps({'rounds': a.rounds, 'missedAtOnce': missed, 'conclusive': conclusive}), flush=True)
    return 1 if missed else 0 if conclusive else 2


sys.exit(main())
