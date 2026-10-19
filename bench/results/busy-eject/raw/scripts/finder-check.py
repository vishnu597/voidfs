#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Hold a disposable acknowledged save for the operator's native Finder eject check."""
import importlib.util
import json
import os
from pathlib import Path
import sys
import time

import rig as r

spec = importlib.util.spec_from_file_location('matrix', Path(__file__).with_name('live-matrix.py'))
matrix = importlib.util.module_from_spec(spec)
spec.loader.exec_module(matrix)
key = 'held-file-finder.bin'
ready = r.R / 'finder-ready.json'
release = r.R / 'finder-release'

if sys.argv[1:] == ['hold']:
    release.unlink(missing_ok=True)
    ready.unlink(missing_ok=True)
    matrix.command('mount-for-finder', [r.VOID, 'mount', r.drive, r.MNT])
    matrix.offline()
    device = os.stat(r.MNT).st_dev
    fd, data = matrix.write_open(key, 131111)
    ready.write_text(json.dumps({'device': device, 'key': key}))
    try:
        r.wait(release.exists, timeout=240, what='operator finished Finder check')
    finally:
        os.close(fd)
        matrix.emit('finder-holder-closed', key=key)
elif sys.argv[1:] == ['recover']:
    value = json.loads(ready.read_text())
    matrix.command('reconcile-finder-eject', [r.VOID, 'unmount', r.MNT])
    matrix.remount_and_check(key, (r.R / 'seeds' / key).read_bytes(), value['device'])
    matrix.command('normal-eject-after-finder-recovery', [r.VOID, 'unmount', r.MNT])
else:
    raise SystemExit('usage: finder-check.py hold | recover')
