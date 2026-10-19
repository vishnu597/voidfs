#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Operator-run live checks; all data and credentials belong to the disposable rig."""

import argparse
import hashlib
import json
import os
import sqlite3
import threading
import time

import rig as r


def emit(event, **fields):
    record = r.emit(event, **fields)
    print(json.dumps(record, sort_keys=True), flush=True)


def optional(call):
    try:
        return call()
    except (OSError, RuntimeError, TimeoutError):
        return None


def info():
    return r.ctl("GET", "/v1/info")


def status():
    return r.ctl("GET", "/v1/status")


def link():
    value = optional(status)
    return value and value["connection"]["link"]


def mounted():
    value = optional(lambda: r.ctl("GET", "/v1/mounts"))
    if not value:
        return None
    for mount in value["mounts"]:
        if mount["mountpoint"] == str(r.MNT):
            if mount["state"] == "failed":
                raise RuntimeError("remembered mount failed: " + str(mount["error"]))
            return mount if mount["state"] == "mounted" else None
    return None


def row(key):
    found = [entry for entry in r.entries() if entry["key"] == key]
    return found[-1] if found else None


def wait_row(key, state, timeout=90):
    def ready():
        value = row(key)
        return value if value and value["state"] == state else None
    return r.wait(ready, timeout=timeout, interval=0.05, what=key + " " + state)


def payload(key, size=262177):
    path = r.R / "seeds" / key
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(os.urandom(size))
    return path


def sha(data):
    return hashlib.sha256(data).hexdigest()


def write_mount(source, key):
    data = source.read_bytes()
    count = (r.MNT / key).write_bytes(data)
    assert count == len(data)
    emit("mount-write-acknowledged", key=key, bytes=count, sha256=sha(data))


def verify_peer(source, key):
    destination = r.R / "peer-read.bin"
    destination.unlink(missing_ok=True)
    r.peer("show", r.drive, key, "--output", destination)
    expected, actual = source.read_bytes(), destination.read_bytes()
    assert actual == expected, "peer bytes differ for " + key
    emit("peer-bytes-verified", key=key, bytes=len(actual), sha256=sha(actual))


def restart(label):
    before = info()
    old_device = os.stat(r.MNT).st_dev
    start = time.monotonic()
    result = r.run([r.VOID, "daemon", "restart"], timeout=90, check=False)
    emit("restart-command", phase=label, exit=result.returncode,
         stdout=result.stdout, stderr=result.stderr)
    fresh = r.wait(lambda: optional(lambda: (v if v["pid"] != before["pid"] else None)
                                    if (v := info()) else None),
                   timeout=45, what="new daemon PID")
    mount = r.wait(mounted, timeout=65, what="remembered FSKit mount")
    new_device = os.stat(r.MNT).st_dev
    assert old_device != new_device, "a fresh mount is needed to exclude old kernel pages"
    emit("daemon-and-mount-restored", phase=label, old_pid=before["pid"], pid=fresh["pid"],
         old_device=old_device, device=new_device, mount=mount,
         seconds=round(time.monotonic() - start, 3), command_exit=result.returncode)
    return fresh


def configure():
    # Confirm proxy signing/forwarding before putting its endpoint in the Keychain.
    smoke = r.run([r.VOID, "--json", "--endpoint", r.ENDPOINT, "--key-file", r.KEY,
                   "drive", "show", r.drive], state=r.C)
    emit("signed-proxy-smoke", result=json.loads(smoke.stdout))
    r.run(["/usr/bin/defaults", "write", "dev.voidfs.app", "VoidfsStateDirectory", "-string", r.D])
    signed = r.run([r.APP, "server-key", "sign-in", "--endpoint", r.ENDPOINT, "--key-file", r.KEY], timeout=45)
    emit("throwaway-sign-in", stdout=signed.stdout)
    r.run(["/bin/launchctl", "kickstart", "-k", "gui/%s/dev.voidfs.agent" % os.getuid()])
    enabled = r.run([r.APP, "login-item", "on"], timeout=45)
    emit("login-item-on", stdout=enabled.stdout)
    r.wait(lambda: optional(info), timeout=30, what="login daemon")
    mount = r.run([r.VOID, "mount", r.drive, r.MNT], timeout=75)
    emit("initial-mount", stdout=mount.stdout, mount=r.wait(mounted, timeout=15))
    for key in ["baseline-root.txt", "nested/baseline-nested.txt"]:
        expected = (r.R / "seeds" / key).read_bytes()
        actual = (r.MNT / key).read_bytes()
        assert actual == expected
        emit("baseline-read", key=key, sha256=sha(actual), bytes=len(actual))
    emit("configured", info=info(), status=status())


def retry():
    own = r.R / "seeds/own-uploaded-never-read.bin"
    if not own.exists():
        own = payload(own.name)
        write_mount(own, own.name)
    done = wait_row(own.name, "done")
    verify_peer(own, own.name)
    # No read of own through the mount: the later first read must exercise publication seeding.
    emit("own-save-published-without-mount-read", entry=done)
    r.mode(fail_mutations=True)
    source = payload("retry-after-restart.bin", 131111)
    write_mount(source, source.name)
    before = wait_row(source.name, "failed")
    assert before["retry_at"] is not None and before["source"]
    emit("transient-failure-durable", entry=before, link=link())
    restart("failed-upload")
    after = wait_row(source.name, "failed")
    assert after["id"] == before["id"] and after["retry_at"] is not None
    emit("transient-failure-after-restart", entry=after, link=link())
    r.mode()
    done = wait_row(source.name, "done")
    verify_peer(source, source.name)
    emit("retry-without-resume-passed", entry=done)


def offline():
    r.stop_proxy()
    r.wait(lambda: link() == "offline", timeout=35, what="offline connectivity")
    source = payload("queued-during-offline.bin", 98357)
    write_mount(source, source.name)
    before = wait_row(source.name, "queued", timeout=15)
    assert before["error"] is None and not before["paused"] and before["source"]
    emit("offline-save-queued", entry=before)
    transitions = []
    observer_errors = []
    stop = threading.Event()
    ready = threading.Event()
    def observe():
        try:
            database = (r.D / "state.sqlite").resolve().as_uri() + "?mode=ro"
            # Keep the read connection across restart; reopening it while SQLite closes
            # its last writer can briefly race removal/recreation of the WAL sidecars.
            with sqlite3.connect(database, uri=True, timeout=5) as connection:
                connection.row_factory = sqlite3.Row
                while not stop.is_set():
                    value = connection.execute("SELECT id,state,error,retry_at FROM entries WHERE id=?", (before["id"],)).fetchone()
                    if value:
                        current = dict(value)
                        if not transitions or transitions[-1] != current:
                            transitions.append(current)
                        ready.set()
                    stop.wait(0.025)
        except Exception as error:
            observer_errors.append(type(error).__name__)
            ready.set()
    observer = threading.Thread(target=observe)
    observer.start()
    try:
        assert ready.wait(5) and not observer_errors
        restart("server-offline")
        r.wait(lambda: link() == "offline", timeout=20, what="offline after startup probe")
        time.sleep(2)
    finally:
        stop.set()
        observer.join(timeout=5)
    after = row(source.name)
    assert not observer_errors, observer_errors
    assert after["id"] == before["id"] and after["state"] == "queued" and after["error"] is None
    assert all(value["state"] == "queued" and value["error"] is None for value in transitions)
    emit("queued-through-unknown-startup-passed", before=before, after=after, transitions=transitions)
    own = r.R / "seeds/own-uploaded-never-read.bin"
    actual = (r.MNT / own.name).read_bytes()
    assert actual == own.read_bytes()
    emit("uploaded-save-offline-read-passed", key=own.name,
         bytes=len(actual), sha256=sha(actual), link=link(), info=info())


def online():
    r.mode()
    r.start_proxy()
    r.wait(lambda: link() == "online", timeout=45, what="first online after offline restart")
    emit("first-online", info=info())
    for key in ["first-online-root.txt", "nested/first-online-child.txt"]:
        source = payload(key, 8193)
        target = r.drive + ":/" + ("nested/" if key.startswith("nested/") else "")
        r.peer("upload", source, target)
        start = time.monotonic()
        parent = (r.MNT / key).parent
        r.wait(lambda: source.name in os.listdir(parent), timeout=8, interval=0.025,
               what=key + " in containing listing")
        visible = time.monotonic() - start
        actual = (r.MNT / key).read_bytes()
        assert actual == source.read_bytes()
        emit("first-online-name", key=key, listing_seconds=round(visible, 4),
             first_path_read_sha256=sha(actual), within_five_seconds=visible <= 5)
        assert visible <= 5
    queued = r.R / "seeds/queued-during-offline.bin"
    done = wait_row(queued.name, "done")
    verify_peer(queued, queued.name)
    emit("offline-queued-published-without-resume-passed", entry=done)
    for command in [[r.APP, "agent-status"], [r.APP, "login-item", "status"]]:
        result = r.run(command)
        assert "refuses" not in result.stdout
        if command[-1] == "agent-status":
            assert "enabled" in result.stdout
        else:
            assert "login item: on:" in result.stdout
        emit("live-job-status", command=[str(x) for x in command], stdout=result.stdout)
    emit("primary-live-checks-passed", info=info(), status=status())


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("phase", choices=["configure", "retry", "offline", "online"])
    args = parser.parse_args()
    globals()[args.phase]()
