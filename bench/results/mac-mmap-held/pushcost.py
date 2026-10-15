#!/usr/bin/python3
# SPDX-License-Identifier: Apache-2.0
"""Hold COUNT writable mappings, first clean then dirty, for timer-cost measurements.

Run only on an isolated test mount. Each run owns a new child directory. The first
generation is explicitly msync'ed to make the clean interval unambiguous; the second
generation has no explicit flush. Match the monotonic/wall timestamps to FSKit's trace.
"""
import argparse
import contextlib
import json
import mmap
import os
from pathlib import Path
import resource
import tempfile
import time


def emit(phase, **values):
    print(json.dumps(dict(phase=phase, monotonic_ns=time.monotonic_ns(),
                          unix_ns=time.time_ns(), **values)), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("count", type=int)
    parser.add_argument("--hold", type=float, default=5)
    args = parser.parse_args()
    if not 0 <= args.count <= 1000 or not 1 <= args.hold <= 60:
        parser.error("count must be 0..1000 and hold 1..60 seconds")
    soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
    needed = 2 * args.count + 32  # Python mmap retains its own duplicate descriptor.
    if hard != resource.RLIM_INFINITY and needed > hard:
        parser.error("hard descriptor limit is too small for this count")
    if soft != resource.RLIM_INFINITY and soft < needed:
        resource.setrlimit(resource.RLIMIT_NOFILE, (needed, hard))
    root = Path(tempfile.mkdtemp(prefix="push-cost-", dir=args.root))
    mappings = []
    emit("start", count=args.count, root=str(root), page_size=mmap.PAGESIZE)
    with contextlib.ExitStack() as resources:
        for index in range(args.count):
            fd = os.open(root / ("file-%04d" % index), os.O_CREAT | os.O_EXCL | os.O_RDWR, 0o600)
            resources.callback(os.close, fd)
            os.ftruncate(fd, mmap.PAGESIZE)
            mapping = mmap.mmap(fd, mmap.PAGESIZE, access=mmap.ACCESS_WRITE)
            resources.callback(mapping.close)
            mappings.append(mapping)
            mapping[0] = 1
            mapping.flush()  # explicit control, not part of the unsynchronized measurement
        emit("clean-held", count=len(mappings), explicit_msync=True)
        time.sleep(args.hold)
        emit("dirty-start", count=len(mappings))
        for mapping in mappings:
            mapping[0] = 2
        emit("dirty-held", count=len(mappings), explicit_msync=False)
        time.sleep(args.hold)
    emit("closed", count=len(mappings), root=str(root))


if __name__ == "__main__":
    main()
