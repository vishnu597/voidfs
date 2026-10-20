#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Shared recovery observations for the installed native-UI force-eject check.

Importing this module performs no actions. The UI requests force eject itself;
these helpers observe the result and check offline/peer byte recovery.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import time

import rig as r


def emit(event, **fields):
    record = r.emit(event, **fields)
    print(json.dumps(record, sort_keys=True), flush=True)
    return record


def command(label, args, timeout=75):
    started = time.monotonic()
    result = r.run(args, timeout=timeout, check=False)
    emit("command", label=label, args=[str(arg) for arg in args], exit=result.returncode,
         stdout=result.stdout, stderr=result.stderr,
         seconds=round(time.monotonic() - started, 4))
    if result.returncode:
        raise RuntimeError(label + " failed")
    return result


def mounts():
    return r.ctl("GET", "/v1/mounts", timeout=5)


def info():
    return r.ctl("GET", "/v1/info", timeout=5)


def link():
    return r.ctl("GET", "/v1/status", timeout=5)["connection"]["link"]


def mounted():
    return next((item for item in mounts()["mounts"]
                 if item["mountpoint"] == str(r.MNT) and item["state"] == "mounted"), None)


def os_mount():
    table = r.run(["/sbin/mount"], timeout=10).stdout
    suffix = " on " + str(r.MNT) + " ("
    return next((line for line in table.splitlines() if suffix in line), None)


def absent(value):
    return not value["mounts"] and not value["remembered"]


def row(key):
    return next((entry for entry in reversed(r.entries()) if entry["key"] == key), None)


def queued(key, expected_size):
    value = row(key)
    if (value and value["state"] == "queued" and value["error"] is None
            and not value["paused"] and value["size"] == expected_size and value["source"]):
        return value
    return None


def wait_queued(key, expected_size):
    value = r.wait(lambda: queued(key, expected_size), timeout=20, interval=0.05,
                   what="durable queued bytes for " + key)
    emit("queued-offline-save", key=key, entry=value)
    return value


def ensure_mounted():
    value = mounts()
    if absent(value):
        command("mount-for-case", [r.VOID, "mount", r.drive, r.MNT])
    r.wait(mounted, timeout=75, what="disposable FSKit mount")
    if not os_mount():
        raise RuntimeError("daemon mount has no matching operating-system volume")
    if Path.cwd() == r.MNT or r.MNT in Path.cwd().parents:
        raise RuntimeError("run the controller outside the mounted volume")


def write_open(key, size):
    path = r.R / "seeds" / key
    if path.exists() or row(key) is not None:
        raise RuntimeError("use a fresh rig: payload name was already used: " + key)
    data = os.urandom(size)
    path.write_bytes(data)
    descriptor = os.open(r.MNT / key, os.O_CREAT | os.O_EXCL | os.O_RDWR, 0o600)
    try:
        written = 0
        while written < len(data):
            count = os.write(descriptor, data[written:])
            if count <= 0:
                raise RuntimeError("mount write did not advance")
            written += count
    except BaseException:
        os.close(descriptor)
        raise
    emit("mount-write-acknowledged", key=key, bytes=written,
         sha256=hashlib.sha256(data).hexdigest(), entries=r.entries())
    return descriptor, data


def wait_automatically_forgotten(label, since=None):
    started = time.monotonic() if since is None else since
    observations = []

    def cleared():
        value = mounts()
        current = {"live": len(value["mounts"]), "remembered": len(value["remembered"])}
        if not observations or observations[-1]["state"] != current:
            observations.append({"seconds": round(time.monotonic() - started, 4), "state": current})
        return value if absent(value) else None

    value = r.wait(cleared, timeout=20, interval=0.05,
                   what="automatic live and remembered removal after " + label)
    if os_mount() is not None:
        raise RuntimeError("volume is still in the OS mount table after external eject")
    emit("automatically-forgotten", label=label, mounts=value, observations=observations,
         seconds_since_observation_start=round(time.monotonic() - started, 4),
         entries=r.entries())
    return value


def restart_no_remount(label, key, size):
    if not absent(mounts()) or os_mount() is not None:
        raise RuntimeError("external eject has not been automatically reconciled")
    before = info()["pid"]
    command("restart-with-forgotten-intent", [r.VOID, "daemon", "restart"], timeout=90)
    fresh = r.wait(lambda: (value if value["pid"] != before else None)
                   if (value := info()) else None, timeout=30, what="new daemon PID")
    # Check a grace interval as well as the immediate result: restoration is asynchronous.
    deadline = time.monotonic() + 2
    samples = 0
    while time.monotonic() < deadline:
        if not absent(mounts()) or os_mount() is not None:
            raise RuntimeError("external eject unexpectedly remounted after daemon restart")
        samples += 1
        time.sleep(0.1)
    r.wait(lambda: link() == "offline", timeout=30, what="offline after restart")
    entry = wait_queued(key, size)
    emit("forgotten-intent-survives-restart", label=label, old_pid=before,
         pid=fresh["pid"], empty_mount_samples=samples, observation_seconds=2,
         mounts=mounts(), entry=entry)


def check_bytes(key, expected):
    # A bounded child performs mount reads, so an unexpected FSKit hang does not
    # prevent the controller from recording a failed check.
    probe = """import hashlib,json,pathlib,sys
actual=pathlib.Path(sys.argv[1]).read_bytes()
original=pathlib.Path(sys.argv[2]).read_bytes()
print(json.dumps({'matches':actual==original,'bytes':len(actual),'sha256':hashlib.sha256(actual).hexdigest()}))
"""
    result = r.run(["/usr/bin/python3", "-c", probe, r.MNT / key, r.R / "seeds" / key], timeout=20)
    value = json.loads(result.stdout)
    if not value["matches"] or value["bytes"] != len(expected) or value["sha256"] != hashlib.sha256(expected).hexdigest():
        raise RuntimeError("offline mounted bytes differ for " + key)
    return value


def remount_restore_and_publish(label, key, data, old_device):
    if link() != "offline":
        raise RuntimeError("recovery must begin offline")
    command("explicit-offline-remount", [r.VOID, "mount", r.drive, r.MNT])
    r.wait(mounted, timeout=75, what="explicit offline mount")
    device = os.stat(r.MNT).st_dev
    if device == old_device:
        raise RuntimeError("explicit remount retained the old mount device")
    read = check_bytes(key, data)
    emit("exact-bytes-after-explicit-offline-remount", label=label, key=key,
         old_device=old_device, device=device, read=read, entries=r.entries())

    before = info()["pid"]
    command("restart-with-intentionally-remembered-mount", [r.VOID, "daemon", "restart"], timeout=90)
    fresh = r.wait(lambda: (value if value["pid"] != before else None)
                   if (value := info()) else None, timeout=30, what="restored daemon PID")
    restored = r.wait(mounted, timeout=75, what="intentionally remembered offline mount")
    r.wait(lambda: link() == "offline", timeout=30, what="offline restored mount")
    new_device = os.stat(r.MNT).st_dev
    if new_device == device:
        raise RuntimeError("restored mount retained the old kernel device")
    if not any(item["mountpoint"] == str(r.MNT) for item in mounts()["remembered"]):
        raise RuntimeError("intentional daemon restart forgot the mount")
    read = check_bytes(key, data)
    emit("intentional-offline-restoration-passed", label=label, key=key, old_pid=before,
         pid=fresh["pid"], old_device=device, device=new_device, mount=restored,
         read=read, entries=r.entries())

    r.online()
    entry = r.wait(lambda: (value if value["state"] == "done" else None)
                   if (value := row(key)) else None, timeout=90, interval=0.1,
                   what="automatic upload without resume")
    destination = r.R / "peer-read.bin"
    destination.unlink(missing_ok=True)
    r.peer("show", r.drive, key, "--output", destination)
    actual = destination.read_bytes()
    if actual != data:
        raise RuntimeError("peer bytes differ after reconnect for " + key)
    emit("peer-bytes-without-resume", label=label, key=key, entry=entry,
         bytes=len(actual), sha256=hashlib.sha256(actual).hexdigest())
    command("normal-final-cli-eject", [r.VOID, "unmount", r.MNT])
    if not absent(mounts()) or os_mount() is not None:
        raise RuntimeError("final ordinary eject did not forget the mount")

