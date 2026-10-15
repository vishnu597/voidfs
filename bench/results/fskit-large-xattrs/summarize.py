# SPDX-License-Identifier: Apache-2.0
"""Prints the README's result tables from the raw files.

/usr/bin/python3 summarize.py raw/fskit.jsonl raw/fskit-redump.jsonl

A cell is what the destination holds after the mount settled (kept: the source's size and
SHA-256; a prefix; or lost) and whether the operation reported success. Finder Duplicate and copy
out start from the Finder copy on the mount, so where that copy already lost the value they test
nothing new and say so.
"""
import json
import sys

SIZES = [65000, 65537, 262143, 262144, 1048576, 16777216]
OPS = [('finder-copy', 'Finder copy onto the mount'), ('finder-duplicate', 'Finder Duplicate within the mount'),
       ('finder-copy-out', 'Finder copy back out to APFS'), ('cp', '`cp` onto the mount'),
       ('direct-whole', 'one `fsetxattr`, whole'), ('chunks-64k', 'positional `fsetxattr`, 64 KiB pieces'),
       ('chunks-1m', 'positional `fsetxattr`, 1 MiB pieces'), ('namedfork', '`write()` to `..namedfork/rsrc`, 1 MiB pieces')]

want, ran, settled = {}, {}, {}
for line in open(sys.argv[1]):
    r = json.loads(line)
    if r['op'] == 'source':
        want[r['case']] = r
    else:
        ran[(r['case'], r['op'])] = r
for line in open(sys.argv[2]):
    r = json.loads(line)
    settled[(r['case'], r['op'])] = r


def held(case, op):
    w = want[case]
    x = ((settled.get((case, op)) or {}).get('dump') or {}).get('xattrs', {}).get(w['name'])
    if x and x.get('size') == w['bytes'] and x.get('sha256') == w['sha256']:
        return 'kept'
    return f"{x['size']:,} B prefix" if x else '**lost**'


def cell(case, op):
    r = ran.get((case, op))
    if not r:
        return '—'
    if op in ('finder-duplicate', 'finder-copy-out') and held(case, 'finder-copy') != 'kept':
        return 'n/a: source lost it'
    return f"{held(case, op)}, {'success' if r['exit'] == 0 else 'error'}"


for kind, title in [('fork', 'com.apple.ResourceFork'), ('user', 'user.voidfs.large')]:
    print(f'\n`{title}`, value size in bytes:\n')
    print('| Operation | ' + ' | '.join(f'{s:,}' for s in SIZES) + ' |')
    print('|---|' + '---|' * len(SIZES))
    for op, label in OPS:
        if kind == 'user' and op in ('chunks-64k', 'chunks-1m', 'namedfork'):
            continue
        print(f'| {label} | ' + ' | '.join(cell(f'{kind}-{s}', op) for s in SIZES) + ' |')
