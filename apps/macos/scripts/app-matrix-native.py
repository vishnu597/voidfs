#!/usr/bin/python3
# SPDX-License-Identifier: Apache-2.0
"""Native app scratch-file rows. Run with --row inventory|finder|textedit|media.

Each mutating row creates its own child below --root. Use an isolated test mount or APFS
control, never an existing document. --media-bin names the compiled app-matrix-media.swift.
The common app-matrix.py collector separately reconciles returned artifact folders.
"""
import argparse
import hashlib
import json
import importlib.util
import os
from pathlib import Path
import plistlib
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--row', required=True, choices=['inventory', 'finder', 'textedit', 'media'])
    parser.add_argument('--root', required=True)
    parser.add_argument('--out', required=True)
    parser.add_argument('--media-bin', default='/private/tmp/voidfs-media')
    args = parser.parse_args()
    output = Path(args.out)
    output.parent.mkdir(parents=True, exist_ok=True)
    failures = 0
    def emit(value):
        with output.open('a') as log:
            log.write(json.dumps(dict(time=time.time(), **value), ensure_ascii=False) + '\n')
        print(json.dumps(value, ensure_ascii=False), flush=True)
    def run(argv, timeout=90, expect=0):
        nonlocal failures
        started = time.monotonic()
        try:
            result = subprocess.run([str(item) for item in argv], capture_output=True, text=True, timeout=timeout)
            ok = result.returncode == expect
            emit(dict(kind='command', argv=[str(item) for item in argv], rc=result.returncode, stdout=result.stdout, stderr=result.stderr, elapsedSeconds=time.monotonic() - started, ok=ok))
        except subprocess.TimeoutExpired as error:
            ok = False
            emit(dict(kind='command', argv=[str(item) for item in argv], timeout=timeout, ok=False, stdout=str(error.stdout), stderr=str(error.stderr)))
        if not ok:
            failures += 1
        return ok
    def check(name, ok, **detail):
        nonlocal failures
        emit(dict(kind='check', name=name, ok=ok, **detail))
        failures += not ok
    if args.row == 'inventory':
        apps = {'Finder':'/System/Library/CoreServices/Finder.app', 'TextEdit':'/System/Applications/TextEdit.app', 'Preview':'/System/Applications/Preview.app', 'QuickTime Player':'/System/Applications/QuickTime Player.app', 'Photos':'/System/Applications/Photos.app', 'Numbers':'/Applications/Numbers Creator Studio.app', 'Xcode':'/Applications/Xcode-beta.app', 'Zed':'/Applications/Zed.app', 'Cursor':'/Applications/Cursor.app', 'Blender':'/Applications/Blender.app', 'LibreOffice':'/Applications/LibreOffice.app', 'Premiere':'/Applications/Adobe Premiere Pro 2026/Adobe Premiere Pro 2026.app', 'Final Cut':'/Applications/Final Cut Pro.app', 'Resolve':'/Applications/DaVinci Resolve/DaVinci Resolve.app', 'Office Word':'/Applications/Microsoft Word.app'}
        for name, path in apps.items():
            info = Path(path) / 'Contents/Info.plist'
            version = plistlib.loads(info.read_bytes()).get('CFBundleShortVersionString') if info.exists() else None
            emit(dict(kind='app', app=name, path=path, installed=info.exists(), version=version))
        for argv in [['/usr/bin/sw_vers'], ['/usr/bin/git', '--version'], ['/usr/bin/sqlite3', '--version'], ['/bin/df', '-h', '/System/Volumes/Data']]:
            run(argv)
        return failures
    parent = Path(args.root).resolve()
    parent.mkdir(parents=True, exist_ok=True)
    folder = Path(tempfile.mkdtemp(prefix=args.row + '-', dir=parent))
    emit(dict(kind='start', row=args.row, folder=str(folder)))
    script = Path(__file__).with_name('app-matrix-cocoa.applescript')
    if args.row == 'finder':
        spec = importlib.util.spec_from_file_location('matrix_evidence', Path(__file__).with_name('app-matrix.py'))
        evidence = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(evidence)
        source = Path(tempfile.mkdtemp(prefix='voidfs-finder-source.', dir='/private/tmp'))
        tree = source / 'finder-fixture'
        tree.mkdir()
        nested = tree / 'nested' / '名前'
        nested.mkdir(parents=True)
        fixture = tree / 'xattr.bin'
        fixture.write_bytes(bytes(range(256)) * 20480)
        (nested / 'notes.txt').write_text('Finder folder tree — 名前\n')
        os.chmod(fixture, 0o640)
        os.utime(fixture, (1700000123, 1700000123))
        run(['/usr/bin/xattr', '-w', 'user.voidfs.matrix', 'finder-metadata', fixture])
        run(['/usr/bin/xattr', '-w', '-x', 'com.apple.ResourceFork', '666f726b2d6d6174726978', fixture])
        tags = plistlib.dumps(['matrix\n2'], fmt=plistlib.FMT_BINARY)
        run(['/usr/bin/xattr', '-w', '-x', 'com.apple.metadata:_kMDItemUserTags', tags.hex(), fixture])
        exported = source / 'export'
        exported.mkdir()
        sha = hashlib.sha256(fixture.read_bytes()).hexdigest()
        source_xattrs = evidence.xattrs(fixture)
        emit(dict(kind='source', path=str(fixture), sha256=sha, bytes=fixture.stat().st_size))
        if run(['/usr/bin/osascript', script, 'finder-copy', tree, folder, exported]):
            copied = folder / tree.name
            renamed = copied / 'renamed.bin'
            check('copy in bytes', renamed.exists() and hashlib.sha256(renamed.read_bytes()).hexdigest() == sha)
            check('copy out bytes', (exported / 'renamed.bin').exists() and hashlib.sha256((exported / 'renamed.bin').read_bytes()).hexdigest() == sha)
            duplicate = copied / 'duplicate.bin'
            check('duplicate bytes', duplicate.exists() and hashlib.sha256(duplicate.read_bytes()).hexdigest() == sha)
            for target in [renamed, duplicate, exported / 'renamed.bin']:
                attributes = evidence.xattrs(target)
                for name in ['user.voidfs.matrix', 'com.apple.ResourceFork']:
                    check('source metadata round trip', attributes.get(name) == source_xattrs[name], path=str(target), xattr=name)
            check('Finder changed label without dropping source tag', 'com.apple.metadata:_kMDItemUserTags' in evidence.xattrs(renamed))
            check('folder tree bytes', (copied / 'nested/名前/notes.txt').read_text() == 'Finder folder tree — 名前\n')
            # Publication/recovery is checked before calling finder-trash separately.
            thumbs = source / 'quicklook'
            thumbs.mkdir()
            run(['/usr/bin/qlmanage', '-t', '-s', '320', '-o', thumbs, copied / 'nested/名前/notes.txt'])
            emit(dict(kind='artifacts', folder=str(copied), source=str(tree), export=str(exported), trashCommand=['/usr/bin/osascript', str(script), 'finder-trash', str(copied)]))
    elif args.row == 'textedit':
        target = folder / 'notes.txt'
        target.write_text('application matrix source draft\n')
        if run(['/usr/bin/osascript', script, 'textedit', target]):
            expected = 'application matrix edited draft — 名前\n'
            check('TextEdit two safe saves and fresh reopen preserve text', target.read_text() == expected, sha256=hashlib.sha256(target.read_bytes()).hexdigest())
        emit(dict(kind='artifacts', folder=str(folder)))
    elif args.row == 'media':
        movie = folder / 'real-prores.mov'
        if run([args.media_bin, 'make', movie, '300', '1920', '1080', '1313'], timeout=180):
            run([args.media_bin, 'inspect', movie], timeout=180)
            run([args.media_bin, 'inspect', movie], timeout=180)
            export = folder / 'trimmed-hevc.mov'
            run(['/usr/bin/avconvert', '--source', movie, '--output', export, '--preset', 'PresetHEVCHighestQuality', '--start', '1', '--duration', '2', '--replace'], timeout=180)
            if export.exists():
                run([args.media_bin, 'inspect', export], timeout=180)
        emit(dict(kind='artifacts', folder=str(folder)))
    emit(dict(kind='summary', row=args.row, failures=failures, folder=str(folder)))
    return int(bool(failures))


if __name__ == '__main__':
    raise SystemExit(main())
