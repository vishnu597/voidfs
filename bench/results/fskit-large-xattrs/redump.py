# SPDX-License-Identifier: Apache-2.0
"""Reads every destination in a run-cases.py results file again, after the mount has settled.

/usr/bin/python3 redump.py <xattr-probe> <results.jsonl> > <redump.jsonl>

The immediate readback in run-cases.py can run before the module's cached attributes catch up
with a Finder copy (see the README); this one, minutes later, is the one the tables use.
"""
import datetime
import json
import os
import subprocess
import sys

probe, results = sys.argv[1], sys.argv[2]
want = {}
for line in open(results):
    r = json.loads(line)
    if r['op'] == 'source':
        want[r['case']] = r
        continue
    path = r.get('dest')
    if not path or not os.path.exists(path):
        print(json.dumps({'case': r['case'], 'op': r['op'], 'dest': path, 'exists': False}))
        continue
    when = datetime.datetime.now().astimezone().isoformat(timespec='microseconds')
    dump = json.loads(subprocess.run([probe, 'dump', path], capture_output=True, text=True).stdout)
    w = want[r['case']]
    x = dump.get('xattrs', {}).get(w['name'])
    keeps = bool(x and x.get('size') == w['bytes'] and x.get('read') == w['bytes'] and x.get('sha256') == w['sha256'])
    print(json.dumps({'case': r['case'], 'op': r['op'], 'dest': path, 'time': when, 'keeps': keeps, 'dump': dump}))
