# SPDX-License-Identifier: Apache-2.0
"""Joins the module's xattr trace lines to the operations in a run-cases.py results file.

/usr/bin/python3 trace-xattrs.py <fskit-trace.ndjson> <results.jsonl> <mount> > <per-op.jsonl>

The trace is `log stream --style ndjson --predicate 'subsystem == "dev.voidfs"'` of a mount with
`-o trace`. Lines are attributed by inode: on this mount a file's st_ino is the module's inode
number plus 2 (every file in the run). An operation that writes onto the mount gets its
destination's lines from its start to its end ("op") and during the readback dump ("dump"); a
copy out of the mount gets the lines of the mounted source between its start and end. Each
setxattr of the case's attribute is checked against the source value: `whole` when its size and
SHA-256 are the source's. An operation whose destination never appeared on the mount gets the
xattr lines of files created on the mount between its start and end.
"""
import datetime
import json
import re
import sys

SET = re.compile(r'^setxattr (\d+) (\S+) policy (\d+) (-?\d+) bytes sha256 (\S+)$')
SETFAIL = re.compile(r'^setxattr (\d+) (\S+) failed errno (\d+)$')
GET = re.compile(r'^getxattr (\d+) (\S+) (\d+) bytes sha256 (\S+) in (.+)$')
GETFAIL = re.compile(r'^getxattr (\d+) (\S+) failed errno (\d+)$')
GETABSENT = re.compile(r'^getxattr (\d+) (\S+) absent$')
LIST = re.compile(r'^listxattr (\d+) (\d+) names (.*)$')
CREATE = re.compile(r'^create file (\d+) in (\d+) ')
INO_OFFSET = 2


def ts(text):
    text = text.replace(' ', 'T', 1)
    if re.search(r'[+-]\d{4}$', text):
        text = text[:-2] + ':' + text[-2:]
    return datetime.datetime.fromisoformat(text)


def events(path):
    out = []
    for line in open(path, errors='replace'):
        if not line.startswith('{'):
            continue
        try:
            e = json.loads(line)
        except ValueError:
            continue
        msg = e.get('eventMessage', '')
        for kind, rx in [('set', SET), ('setFailed', SETFAIL), ('get', GET), ('getFailed', GETFAIL), ('getAbsent', GETABSENT),
                         ('list', LIST), ('create', CREATE)]:
            m = rx.match(msg)
            if m:
                out.append({'time': e['timestamp'], 't': ts(e['timestamp']), 'kind': kind, 'g': m.groups(), 'ino': int(m.group(1)), 'message': msg})
                break
    return out


def summarize(evs, want, phase):
    sets, gets, fails = [], [], []
    for e in evs:
        g = e['g']
        if e['kind'] == 'set' and g[1] == want['name']:
            size, policy = int(g[3]), int(g[2])
            sets.append({'time': e['time'], 'phase': phase, 'ino': e['ino'], 'policy': policy, 'bytes': size,
                         'whole': size == want['bytes'] and g[4] == want['sha256']})
        elif e['kind'] == 'get' and g[1] == want['name']:
            gets.append({'time': e['time'], 'phase': phase, 'ino': e['ino'], 'bytes': int(g[2]),
                         'whole': int(g[2]) == want['bytes'] and g[3] == want['sha256']})
        elif e['kind'] in ('setFailed', 'getFailed') and g[1] == want['name']:
            fails.append({'time': e['time'], 'phase': phase, 'message': e['message']})
    return sets, gets, fails


def main():
    trace, results, mount = sys.argv[1], sys.argv[2], sys.argv[3].rstrip('/') + '/'
    ev = events(trace)
    copied_ino = {}
    for line in open(results):
        r = json.loads(line)
        if r['op'] == 'source':
            want = {'name': r['name'], 'bytes': r['bytes'], 'sha256': r['sha256']}
            continue
        start, end = ts(r['start']), ts(r['end'])
        dump = r.get('dump') or {}
        dest = r.get('dest') or ''
        sets, gets, fails = [], [], []
        if dest.startswith(mount) and 'ino' in dump:
            ino = dump['ino'] - INO_OFFSET
            if r['op'] == 'finder-copy':
                copied_ino[r['case']] = ino
            mine = [e for e in ev if e['ino'] == ino]
            for phase, lo, hi in [('op', start, end), ('dump', ts(dump['dumpStart']), ts(dump['dumpEnd']))]:
                s, g, f = summarize([e for e in mine if lo <= e['t'] <= hi], want, phase)
                sets += s
                gets += g
                fails += f
            attribution = {'by': 'inode', 'ino': ino}
        elif r['op'] == 'finder-copy-out' and r['case'] in copied_ino:
            ino = copied_ino[r['case']]
            sets, gets, fails = summarize([e for e in ev if e['ino'] == ino and start <= e['t'] <= end], want, 'op')
            attribution = {'by': 'mounted source inode', 'ino': ino}
        else:
            created = {e['ino'] for e in ev if e['kind'] == 'create' and start <= e['t'] <= end}
            sets, gets, fails = summarize([e for e in ev if e['ino'] in created and start <= e['t'] <= end], want, 'op')
            attribution = {'by': 'files created during the operation', 'inos': sorted(created)}
        print(json.dumps({
            'case': r['case'], 'op': r['op'], 'exit': r['exit'], 'expected': want, 'attribution': attribution,
            'setxattr': sets, 'getxattr': gets, 'failures': fails,
            'opSetxattrBytes': [s['bytes'] for s in sets if s['phase'] == 'op' and s['policy'] != 3],
            'opSetxattrWhole': all(s['whole'] for s in sets if s['phase'] == 'op' and s['policy'] != 3) if any(s['phase'] == 'op' and s['policy'] != 3 for s in sets) else None,
            'opGetxattrBytes': sorted({x['bytes'] for x in gets if x['phase'] == 'op'}),
            'dumpGetxattrBytes': sorted({x['bytes'] for x in gets if x['phase'] == 'dump'}),
        }, sort_keys=True))


main()
