#!/usr/bin/python3
# SPDX-License-Identifier: Apache-2.0
"""Another machine's changes to a folder `smbspike.py --local` serves, with `mount_peer`'s commands.

    localpeer.py <folder> <control socket> put <key> <text> | fill <key> <bytes> <byte> | delete <key> | rename <from> <to>

A put writes a new file beside the folder and renames it over the key, as a new version replaces
the old (an open file keeps the one it opened). Then it tells the server what changed, as the
change feed would, and prints the time the change was made, as `mount_peer` prints its commit.
`--quiet` makes the change without telling the server.
"""

import os
import socket
import sys
import time

args = sys.argv[1:]
quiet = "--quiet" in args
args = [a for a in args if a != "--quiet"]
root, control, op, rest = args[0], args[1], args[2], args[3:]
scratch = os.path.join(os.path.dirname(os.path.realpath(root)), ".localpeer")
os.makedirs(scratch, exist_ok=True)
events = []


def path(key):
    return os.path.join(root, key)


def ensure_parents(key):
    parts = key.split("/")[:-1]
    for i in range(len(parts)):
        sub = "/".join(parts[:i + 1])
        if not os.path.isdir(path(sub)):
            os.mkdir(path(sub))
            events.append(("added", sub))


def put(key, data):
    ensure_parents(key)
    existed = os.path.exists(path(key))
    tmp = os.path.join(scratch, f"{os.getpid()}-{time.time_ns()}")
    with open(tmp, "wb") as f:
        f.write(data)
    os.rename(tmp, path(key))
    events.append(("modified" if existed else "added", key))


if op == "put":
    put(rest[0], rest[1].encode())
elif op == "fill":
    put(rest[0], bytes([int(rest[2])]) * int(rest[1]))
elif op == "delete":
    os.unlink(path(rest[0]))
    events.append(("removed", rest[0]))
elif op == "rename":
    ensure_parents(rest[1])
    os.rename(path(rest[0]), path(rest[1]))
    events.append(("renamed", rest[0], rest[1]))
else:
    sys.exit("usage: localpeer.py <folder> <control> put|fill|delete|rename …")
at = time.time()
if not quiet:
    s = socket.socket(socket.AF_UNIX)
    s.connect(control)
    f = s.makefile("rwb")
    for e in events:
        f.write(("\t".join(e) + "\n").encode())
        f.flush()
        f.readline()
    s.close()
print(f"{op} committed at {at:.6f}")
