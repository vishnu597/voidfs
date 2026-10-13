#!/usr/bin/python3
# SPDX-License-Identifier: Apache-2.0
"""Finder Trash/server restore and a fresh over-64-KiB resource-fork copy.

Only new synthetic files are used. The existing approved Finder AppleScript supplies the
native operations; the common collector runs as a subprocess without changing its code.
All fixture trees and failure evidence are retained. Credentials come from the isolated
test environment and are never printed. --out must be a new evidence directory.
"""
import argparse
import ctypes
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--mount', type=Path, required=True)
    parser.add_argument('--drive', required=True)
    parser.add_argument('--void', type=Path, required=True)
    parser.add_argument('--metadata', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    mount = args.mount.absolute()
    output = args.out.absolute()
    output.mkdir(parents=True, exist_ok=False)
    root = Path(tempfile.mkdtemp(prefix='finder-trash-recovery-', dir=mount))
    control = Path(tempfile.mkdtemp(prefix='voidfs-finder-recovery-control.', dir='/private/tmp'))
    scripts = Path(__file__).absolute().parent
    spec = importlib.util.spec_from_file_location('matrix_finder_evidence', scripts / 'app-matrix.py')
    evidence = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(evidence)
    cocoa = output / 'finder-recovery-cocoa.applescript'
    source_script = (scripts / 'app-matrix-cocoa.applescript').read_text()
    old_reference = 'delete file "duplicate.bin" of folder (POSIX file (item 2 of argv))'
    new_reference = 'delete ((POSIX file ((item 2 of argv) & "/duplicate.bin")) as alias)'
    if source_script.count(old_reference) != 1:
        raise RuntimeError('Finder Trash harness reference changed; review the approved source before running')
    cocoa.write_text(source_script.replace(old_reference, new_reference))
    log = (output / 'results.jsonl').open('x', encoding='utf-8')
    failures = 0
    results = {}
    libc = ctypes.CDLL(None, use_errno=True)
    libc.setxattr.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_uint32, ctypes.c_int]
    libc.setxattr.restype = ctypes.c_int

    def emit(kind, **detail):
        record = dict(kind=kind, epoch=time.time(), **detail)
        log.write(json.dumps(record, ensure_ascii=False, sort_keys=True) + '\n')
        log.flush()
        if kind in ('check', 'summary', 'error', 'fixture'):
            print(json.dumps(record, ensure_ascii=False, sort_keys=True), flush=True)

    def check(name, ok, **detail):
        nonlocal failures
        failures += not ok
        emit('check', name=name, ok=bool(ok), **detail)
        return ok

    def run(name, command, expected=(0,), binary=False, quiet=False):
        start = time.monotonic()
        result = subprocess.run([str(item) for item in command], stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, timeout=180)
        record = dict(name=name, argv=[str(item) for item in command], rc=result.returncode,
                      seconds=round(time.monotonic() - start, 6), stderr=result.stderr.decode(errors='replace'))
        if binary:
            record.update(size=len(result.stdout), sha256=hashlib.sha256(result.stdout).hexdigest())
        elif not quiet:
            record['stdout'] = result.stdout.decode(errors='replace')
        emit('operation', **record)
        if result.returncode not in expected:
            raise RuntimeError(f'{name}: exit {result.returncode}: {record["stderr"]}')
        return result

    def native_attrs(path, values):
        for name, value in values.items():
            buffer = ctypes.create_string_buffer(value)
            if libc.setxattr(os.fsencode(path), name.encode(), buffer, len(value), 0, 0):
                code = ctypes.get_errno()
                raise OSError(code, os.strerror(code), str(path))

    def fixture(path, data, mode, mtime, attrs):
        path.write_bytes(data)
        native_attrs(path, attrs)
        os.chmod(path, mode)
        os.utime(path, (mtime, mtime))
        record = evidence.metadata(path)
        emit('fixture', **record)
        return record

    def collector(stage):
        run('stable server collector ' + stage,
            [sys.executable, scripts / 'app-matrix.py', '--row', 'collect', '--root', root,
             '--mount', mount, '--drive', args.drive, '--void', args.void, '--metadata', args.metadata,
             '--out', output / (stage + '-server.jsonl')], quiet=True)

    def key(path):
        return str(path.relative_to(mount))

    def history(path):
        return json.loads(run('server history', [args.void, '--json', 'history', args.drive, key(path), '--all']).stdout)['versions']

    def server_attrs(path):
        return json.loads(run('server metadata', [args.metadata, args.drive, 'attrs', key(path)]).stdout)

    def comparable(record):
        return {name: record[name] for name in ('size', 'sha256', 'mode', 'mtime_ns', 'xattrs')}

    def wait_for(name, predicate):
        start = time.monotonic()
        while time.monotonic() - start < 5:
            if predicate():
                return check(name, True, seconds=round(time.monotonic() - start, 6))
            time.sleep(0.05)
        return check(name, False, seconds=round(time.monotonic() - start, 6))

    emit('run', root=str(root), control=str(control), drive=args.drive,
         scope='new synthetic Finder fixture, server version recovery, and large-fork policy')
    emit('harness_adjustment', source=str(scripts / 'app-matrix-cocoa.applescript'), copy=str(cocoa),
         old=old_reference, new=new_reference,
         reason='Finder cannot resolve folder (POSIX file); explicitly coerce only the owned duplicate.bin to an alias')
    try:
        file = root / 'duplicate.bin'
        old = bytes(range(256)) * 256 + 'Finder first revision — 名前\n'.encode()
        edited = b'changed Finder bytes\n' * 4096 + 'second revision — 名前\n'.encode()
        old_attrs = {'user.voidfs.finderRecovery': b'initial metadata', 'com.apple.ResourceFork': b'old finder fork17'}
        new_attrs = {'user.voidfs.finderRecovery': b'edited metadata', 'com.apple.ResourceFork': b'new finder fork17'}
        fixture(control / 'expected-first.bin', old, 0o640, 1700000123, old_attrs)
        fixture(control / 'expected-edited.bin', edited, 0o600, 1700000456, new_attrs)
        fixture(file, old, 0o640, 1700000123, old_attrs)
        collector('initial')
        initial = evidence.metadata(file)
        first_version = next(version['versionId'] for version in history(file) if version.get('isLatest'))
        initial_remote = server_attrs(file)
        check('initial server bytes equal first revision', run('server first bytes', [args.void, 'show', args.drive, key(file)], binary=True).stdout == old)
        fixture(file, edited, 0o600, 1700000456, new_attrs)
        collector('edited')
        check('two revisions differ in bytes and metadata', old != edited and comparable(initial) != comparable(evidence.metadata(file)))
        edited_version = next(version['versionId'] for version in history(file) if version.get('isLatest'))
        check('published history has distinct first and edited versions', first_version != edited_version,
              first_version=first_version, edited_version=edited_version)
        run('Finder Move to Trash', ['/usr/bin/osascript', cocoa, 'finder-trash', root])
        removed = wait_for('Finder removes the scratch path', lambda: not file.exists())
        collector('deleted')
        missing = run('server deleted path', [args.void, 'show', args.drive, key(file)], expected=(1,), binary=True)
        check('deleted server path is absent', missing.returncode == 1)
        deleted = json.loads(run('retained recently deleted entry', [args.metadata, args.drive, 'deleted', key(root) + '/']).stdout)
        entries = deleted if isinstance(deleted, list) else deleted.get('items', [])
        check('deleted object remains recoverable in server history', any(entry.get('key') == key(file) for entry in entries), entries=deleted)
        no_trash = not (mount / '.Trashes').exists()
        emit('policy', name='no volume Trash storage', no_trashes=no_trash,
             meaning='Finder removes the object; server history supplies recovery, rather than a volume Trash folder')
        run('restore first server version', [args.void, '--json', 'restore', args.drive, key(file), '--version', first_version])
        def restored():
            try:
                return file.read_bytes() == old
            except FileNotFoundError:
                return False
        visible = wait_for('restored prior bytes visible in a fresh open within 5 s', restored)
        restored_local = evidence.metadata(file) if file.exists() else None
        check('restored mount SHA mode mtime and all xattrs equal first revision',
              restored_local is not None and comparable(restored_local) == comparable(initial),
              initial=initial, restored=restored_local)
        restored_remote = server_attrs(file)
        check('restored server mode mtime and all raw xattrs equal first revision',
              all(restored_remote.get(name) == initial_remote.get(name) for name in ('mode', 'mtime', 'xattrs')),
              initial=initial_remote, restored=restored_remote)
        check('restored server bytes equal first revision', run('server restored bytes', [args.void, 'show', args.drive, key(file)], binary=True).stdout == old)
        collector('restored')
        results['trashRecovery'] = 'pass' if failures == 0 and removed and visible else 'fail'

        tree = control / 'large-fork-source'
        tree.mkdir()
        source = tree / 'xattr.bin'
        fork = bytes(range(256)) * 256 + b'!'
        large_data = b'large fork copy data\n' * 2048
        fixture(source, large_data, 0o640, 1700000789, {'com.apple.ResourceFork': fork, 'user.voidfs.largeFork': b'65537-byte fork source'})
        expected_fork = {'size': len(fork), 'sha256': hashlib.sha256(fork).hexdigest()}
        before_large = failures
        for label, parent in [('APFS', control / 'apfs-copy'), ('FSKit', root / 'large-fork-copy')]:
            parent.mkdir()
            export = control / (label.lower() + '-large-fork-export')
            export.mkdir()
            copied = run(label + ' Finder copy over-64-KiB fork', ['/usr/bin/osascript', scripts / 'app-matrix-cocoa.applescript',
                         'finder-copy', tree, parent, export], expected=(0, 1))
            if label == 'APFS':
                check('APFS Finder large-fork control succeeds', copied.returncode == 0)
            else:
                emit('policy', name='FSKit Finder over-64-KiB copy return', rc=copied.returncode,
                     meaning='explicit refusal or silent fork loss is preserved as a failed required copy, never a compatibility pass')
            for target in [parent / tree.name / 'renamed.bin', parent / tree.name / 'duplicate.bin', export / 'renamed.bin']:
                record = evidence.metadata(target) if target.exists() else None
                emit('large_fork_result', surface=label, source=str(source), destination=str(target), expected_fork=expected_fork, actual=record)
                check(label + ' Finder copied data bytes', record is not None and record.get('sha256') == hashlib.sha256(large_data).hexdigest(), path=str(target))
                check(label + ' Finder preserves the complete 65537-byte resource fork',
                      record is not None and record.get('xattrs', {}).get('com.apple.ResourceFork') == expected_fork, path=str(target))
        results['largeForkCopy'] = 'pass' if failures == before_large else 'fail'
        collector('after-large-fork')
    except Exception as error:
        failures += 1
        emit('error', exception=type(error).__name__, message=str(error))
    finally:
        emit('summary', root=str(root), control=str(control), failures=failures, results=results,
             result='pass' if failures == 0 else 'fail', retained=True)
        log.close()
    return 0 if failures == 0 else 1


if __name__ == '__main__':
    sys.exit(main())
