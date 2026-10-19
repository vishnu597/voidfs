#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Run only against rig.py's mounted disposable drive; never touches user data."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import time

import rig as r

SWIFT = Path('/private/tmp/voidfs-busy-eject-swift/voidfs')


def emit(event, **fields):
    print(json.dumps(r.emit(event, **fields), sort_keys=True), flush=True)


def status():
    return r.ctl('GET', '/v1/status')


def mounts():
    return r.ctl('GET', '/v1/mounts')


def command(label, args, success=True, timeout=55):
    start = time.monotonic()
    result = r.run(args, timeout=timeout, check=False)
    emit('command', label=label, exit=result.returncode, stdout=result.stdout,
         stderr=result.stderr, seconds=round(time.monotonic() - start, 3))
    assert (result.returncode == 0) == success, label
    return result


def retained(label):
    value = mounts()
    assert any(m['mountpoint'] == str(r.MNT) and m['state'] == 'mounted' for m in value['mounts'])
    assert any(m['mountpoint'] == str(r.MNT) for m in value['remembered'])
    emit('busy-refusal-keeps-live-and-remembered', label=label, mounts=value)


def offline():
    r.stop_proxy()
    r.wait(lambda: status()['connection']['link'] == 'offline', timeout=35, what='offline')


def write_open(key, size=262177):
    data = os.urandom(size)
    source = r.R / 'seeds' / key
    source.write_bytes(data)
    fd = os.open(r.MNT / key, os.O_CREAT | os.O_TRUNC | os.O_RDWR, 0o600)
    count = 0
    while count < len(data):
        n = os.write(fd, data[count:])
        assert n > 0
        count += n
    emit('write-acknowledged', key=key, bytes=count, sha256=hashlib.sha256(data).hexdigest())
    return fd, data


def remount_and_check(key, data, old_device):
    command('explicit-offline-remount', [r.VOID, 'mount', r.drive, r.MNT], timeout=75)
    device = os.stat(r.MNT).st_dev
    assert device != old_device, 'exclude old kernel cache by requiring a new mount'
    actual = (r.MNT / key).read_bytes()
    assert actual == data
    emit('offline-bytes-on-fresh-mount', key=key, bytes=len(actual), device=device,
         old_device=old_device, sha256=hashlib.sha256(actual).hexdigest(), entries=r.entries())
    # The new daemon must load durable stage state, rather than reuse the old Core.
    before = r.ctl('GET', '/v1/info')['pid']
    command('offline-daemon-restart', [r.VOID, 'daemon', 'restart'], timeout=90)
    after = r.ctl('GET', '/v1/info')['pid']
    assert before != after
    r.wait(lambda: any(m['mountpoint'] == str(r.MNT) and m['state'] == 'mounted'
                       for m in mounts()['mounts']), timeout=75, what='restored FSKit mount')
    fresh = os.stat(r.MNT).st_dev
    assert device != fresh
    assert (r.MNT / key).read_bytes() == data
    emit('offline-bytes-after-daemon-restart', key=key, old_pid=before, pid=after,
         old_device=device, device=fresh, entries=r.entries())
    r.start_proxy()
    r.wait(lambda: status()['connection']['link'] == 'online', timeout=45, what='online')
    def done():
        latest = next((e for e in reversed(r.entries()) if e['key'] == key), None)
        return latest if latest and latest['state'] == 'done' else None
    entry = r.wait(done, timeout=90, what='automatic publication of ' + key)
    destination = r.R / 'peer-read.bin'
    destination.unlink(missing_ok=True)
    r.peer('show', r.drive, key, '--output', destination)
    assert destination.read_bytes() == data
    emit('peer-bytes-without-resume', key=key, bytes=len(data), entry=entry,
         sha256=hashlib.sha256(data).hexdigest())


def held_file(force_via):
    key = 'held-file-' + force_via + '.bin'
    offline()
    old_device = os.stat(r.MNT).st_dev
    fd, data = write_open(key)
    try:
        result = command('normal-cli-refuses-held-file', [r.VOID, 'unmount', r.MNT], success=False)
        assert 'busy' in (result.stdout + result.stderr).lower()
        retained(key)
        command('normal-diskutil-refuses-held-file', ['/usr/sbin/diskutil', 'unmount', r.MNT], success=False)
        retained(key)
        command('normal-menu-model-refuses-held-file', [SWIFT, 'transfers-selftest', r.drive,
                '--eject-busy', '-VoidfsStateDirectory', r.D])
        retained(key)
        assert os.pread(fd, len(data), 0) == data
        if force_via == 'cli':
            command('explicit-cli-force', [r.VOID, 'unmount', r.MNT, '--force'])
        else:
            command('explicit-menu-model-force', [SWIFT, 'transfers-selftest', r.drive,
                    '--force-eject', '-VoidfsStateDirectory', r.D])
        value = mounts()
        assert not value['mounts'] and not value['remembered']
        try:
            read = os.pread(fd, 32, 0)
            emit('old-fd-after-force', key=key, read_bytes=len(read), cached_prefix_matches=read == data[:32])
        except OSError as error:
            emit('old-fd-after-force', key=key, errno=error.errno, message=str(error))
    finally:
        try:
            os.close(fd)
        except OSError as error:
            emit('old-fd-close-after-force', key=key, errno=error.errno)
    remount_and_check(key, data, old_device)
    command('normal-cli-after-handles-close', [r.VOID, 'unmount', r.MNT])
    assert not mounts()['remembered']
    command('mount-for-next-row', [r.VOID, 'mount', r.drive, r.MNT])


def held_cwd():
    offline()
    old_device = os.stat(r.MNT).st_dev
    key = 'held-cwd-diskutil.bin'
    fd, data = write_open(key, 98357)
    os.close(fd)
    marker = r.R / 'cwd-ready'
    marker.unlink(missing_ok=True)
    child = subprocess.Popen(['/usr/bin/python3', '-c',
        'import os,pathlib,sys,time; os.chdir(sys.argv[1]); pathlib.Path(sys.argv[2]).touch(); time.sleep(180)',
        str(r.MNT), str(marker)], env=r.safe_env(), stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        r.wait(marker.exists, timeout=10, what='held cwd')
        command('normal-cli-refuses-held-cwd', [r.VOID, 'unmount', r.MNT], success=False)
        retained(key)
        command('normal-diskutil-refuses-held-cwd', ['/usr/sbin/diskutil', 'unmount', r.MNT], success=False)
        retained(key)
        command('explicit-diskutil-force', ['/usr/sbin/diskutil', 'unmount', 'force', r.MNT])
        # Out-of-band eject does not notify the daemon's mount-intent table. A normal daemon
        # unmount reconciles the gone volume and forgets it without forcing anything else.
        emit('out-of-band-eject-before-reconcile', mounts=mounts())
        command('forget-already-ejected-volume', [r.VOID, 'unmount', r.MNT])
        assert not mounts()['mounts'] and not mounts()['remembered']
    finally:
        child.terminate()
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=5)
        marker.unlink(missing_ok=True)
    remount_and_check(key, data, old_device)
    command('normal-final-eject', [r.VOID, 'unmount', r.MNT])
    emit('live-matrix-passed', status=status(), mounts=mounts())


if __name__ == '__main__':
    held_file('cli')
    held_file('menu')
    held_cwd()
