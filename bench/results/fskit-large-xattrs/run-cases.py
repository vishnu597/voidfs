# SPDX-License-Identifier: Apache-2.0
"""Copies files with large xattr values onto, within and off a folder, and records what arrives.

/usr/bin/python3 run-cases.py --surface apfs|fskit --src <apfs dir> --dest <dir> --export <apfs dir>
    --probe <xattr-probe> --script finder.applescript --out <results.jsonl> [--sizes 65000,65537,...]

For each size, two APFS sources: fork-<size>.bin with com.apple.ResourceFork and user-<size>.bin
with user.voidfs.large, each with a 26-byte user.voidfs.sentinel. Then, into <dest>: Finder copy,
Finder duplicate of that copy, Finder copy of it back out to <export>, `cp`, one whole fsetxattr,
and, for forks, positional fsetxattr in 64 KiB and 1 MiB chunks and writes to ..namedfork/rsrc.
Every destination is read back whole and its SHA-256 compared with the source value's.
Each record carries wall-clock start/end, to slice the module trace.
"""
import argparse
import datetime
import json
import os
import subprocess
import time

FORK = 'com.apple.ResourceFork'
USER = 'user.voidfs.large'


def now():
    return datetime.datetime.now().astimezone().isoformat(timespec='microseconds')


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--surface', required=True)
    ap.add_argument('--src', required=True)
    ap.add_argument('--dest', required=True)
    ap.add_argument('--export', required=True)
    ap.add_argument('--probe', required=True)
    ap.add_argument('--script', required=True)
    ap.add_argument('--out', required=True)
    ap.add_argument('--sizes', default='65000,65537,1048576,16777216')
    ap.add_argument('--kinds', default='fork,user')
    ap.add_argument('--ops', default='finder-copy,finder-duplicate,finder-copy-out,cp,direct-whole,chunks-64k,chunks-1m,namedfork')
    a = ap.parse_args()
    out = open(a.out, 'a')
    ops = a.ops.split(',')

    def emit(record):
        record['surface'] = a.surface
        print(json.dumps(record, sort_keys=True), file=out, flush=True)
        brief = {k: record.get(k) for k in ('case', 'op', 'exit', 'keeps', 'seconds')}
        print(json.dumps(brief), flush=True)

    def run(argv, timeout=900):
        start, t0 = now(), time.monotonic()
        try:
            p = subprocess.run(argv, capture_output=True, text=True, timeout=timeout)
            code, so, se = p.returncode, p.stdout, p.stderr
        except subprocess.TimeoutExpired as e:
            code, so, se = 'timeout', e.stdout or '', e.stderr or ''
        return {'argv': [str(x) for x in argv], 'start': start, 'end': now(), 'seconds': round(time.monotonic() - t0, 3),
                'exit': code, 'stdout': so if isinstance(so, str) else so.decode(errors='replace'),
                'stderr': se if isinstance(se, str) else se.decode(errors='replace')}

    def dump(path):
        r = run([a.probe, 'dump', path], timeout=900)
        try:
            d = json.loads(r['stdout'].strip().splitlines()[-1])
        except Exception:
            d = {'error': r['stdout'][-500:] + r['stderr'][-500:]}
        try:
            st = os.lstat(path)
            d['ino'] = st.st_ino
        except OSError as e:
            d['statErrno'] = e.errno
        d['dumpStart'], d['dumpEnd'] = r['start'], r['end']
        return d

    def judge(record, path, name, size, want, src_dump):
        if path and os.path.exists(path):
            d = dump(path)
            record['dest'] = path
            record['dump'] = d
            x = d.get('xattrs', {}).get(name)
            record['keeps'] = bool(x and x.get('size') == size and x.get('read') == size and x.get('sha256') == want)
            record['dataMatches'] = d.get('dataSha256') == src_dump.get('dataSha256')
            record['sentinel'] = 'user.voidfs.sentinel' in d.get('xattrs', {})
        else:
            record['dest'] = path
            record['keeps'] = False
        return record

    for directory in ['finder-in', 'cp-in', 'direct', 'chunks-64k', 'chunks-1m', 'namedfork']:
        os.makedirs(os.path.join(a.dest, directory), exist_ok=True)
    os.makedirs(os.path.join(a.export, 'finder-out'), exist_ok=True)

    for size in [int(s) for s in a.sizes.split(',')]:
        for kind in a.kinds.split(','):
            name, seed = (FORK, size * 10 + 1) if kind == 'fork' else (USER, size * 10 + 2)
            case = f'{kind}-{size}'
            base = case + '.bin'
            src = os.path.join(a.src, base)
            made = run([a.probe, 'make', src, name, str(size), str(seed)])
            want = json.loads(run([a.probe, 'value', str(size), str(seed)])['stdout'])['sha256']
            src_dump = dump(src)
            emit({'case': case, 'op': 'source', 'name': name, 'bytes': size, 'seed': seed, 'sha256': want, **made,
                  'dump': src_dump, 'keeps': src_dump.get('xattrs', {}).get(name, {}).get('sha256') == want})

            copied = None
            if 'finder-copy' in ops:
                r = run(['/usr/bin/osascript', a.script, 'copy', src, os.path.join(a.dest, 'finder-in')])
                copied = r['stdout'].strip() or os.path.join(a.dest, 'finder-in', base)
                emit(judge({'case': case, 'op': 'finder-copy', 'name': name, 'bytes': size, **r}, copied, name, size, want, src_dump))
            if 'finder-duplicate' in ops and copied and os.path.exists(copied):
                r = run(['/usr/bin/osascript', a.script, 'duplicate', copied])
                emit(judge({'case': case, 'op': 'finder-duplicate', 'name': name, 'bytes': size, 'from': copied, **r},
                           r['stdout'].strip() or None, name, size, want, src_dump))
            if 'finder-copy-out' in ops and copied and os.path.exists(copied):
                r = run(['/usr/bin/osascript', a.script, 'copy', copied, os.path.join(a.export, 'finder-out')])
                emit(judge({'case': case, 'op': 'finder-copy-out', 'name': name, 'bytes': size, 'from': copied, **r},
                           r['stdout'].strip() or os.path.join(a.export, 'finder-out', base), name, size, want, src_dump))
            if 'cp' in ops:
                target = os.path.join(a.dest, 'cp-in', base)
                r = run(['/bin/cp', src, target])
                emit(judge({'case': case, 'op': 'cp', 'name': name, 'bytes': size, **r}, target, name, size, want, src_dump))
            if 'direct-whole' in ops:
                target = os.path.join(a.dest, 'direct', base)
                r = run([a.probe, 'make', target, name, str(size), str(seed)])
                emit(judge({'case': case, 'op': 'direct-whole', 'name': name, 'bytes': size, **r}, target, name, size, want, src_dump))
            if kind == 'fork':
                for op, folder, how in [('chunks-64k', 'chunks-64k', 'chunks:65536'), ('chunks-1m', 'chunks-1m', 'chunks:1048576'),
                                        ('namedfork', 'namedfork', 'rsrcfd:1048576')]:
                    if op not in ops:
                        continue
                    target = os.path.join(a.dest, folder, base)
                    pre = run([a.probe, 'make', target, 'user.voidfs.placeholder', '1', '0'])
                    r = run([a.probe, 'set', target, name, str(size), str(seed), how])
                    r['prepare'] = {k: pre[k] for k in ('exit', 'stdout', 'stderr', 'start', 'end')}
                    emit(judge({'case': case, 'op': op, 'name': name, 'bytes': size, **r}, target, name, size, want, src_dump))


main()
