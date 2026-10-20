#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Observe the native menu's Force Eject Cancel/Confirm flow on private data.

The operator performs every UI action. hold keeps an acknowledged offline writer
open for up to five minutes. cancel-check proves Cancel left the same mount and
bytes available. force-check observes successful native confirmation without
issuing any eject request. Release the holder, then recover the exact bytes.
"""

import argparse
import hashlib
import json
import os

import recovery as m
import rig as r


READY = r.R / "ui-ready.json"
RELEASE = r.R / "ui-release"
CONFIRMED = r.R / "ui-force-observed.json"
KEY = "native-force-offline.bin"


def ready():
    r.verify_rig()
    return json.loads(READY.read_text())


def hold():
    r.wait_daemon()
    if r.login_status()["state"] != "on":
        raise RuntimeError("restore/enable the native login job before the eject flow")
    if READY.exists():
        raise RuntimeError("this payload was already used; use a fresh rig for a new run")
    m.ensure_mounted()
    r.offline()
    RELEASE.unlink(missing_ok=True)
    descriptor, data = m.write_open(KEY, 262177)
    value = {"key": KEY, "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest(),
             "device": os.stat(r.MNT).st_dev, "holder_pid": os.getpid(),
             "holder_identity": r._identity(os.getpid()), "released": False}
    r._atomic_json(READY, value)
    m.emit("native-force-holder-ready", **value)
    try:
        r.wait(RELEASE.exists, timeout=300, what="operator releases the native-force writer")
        try:
            prefix = os.pread(descriptor, 32, 0)
            m.emit("held-fd-after-ui-actions", bytes=len(prefix), matches_prefix=prefix == data[:32])
        except OSError as error:
            m.emit("held-fd-after-ui-actions", errno=error.errno)
    finally:
        try:
            os.close(descriptor)
        except OSError as error:
            m.emit("holder-close-error", errno=error.errno)
        value["released"] = True
        r._atomic_json(READY, value)
        m.emit("native-force-holder-closed", key=KEY, released=True)


def cancel_check():
    value = ready()
    if value["released"] or r._identity(value["holder_pid"]) != value["holder_identity"]:
        raise RuntimeError("the original writer is no longer held")
    view = m.mounts()
    if not m.mounted() or not any(item["mountpoint"] == str(r.MNT) for item in view["remembered"]):
        raise RuntimeError("Cancel did not retain the live and remembered mount")
    if os.stat(r.MNT).st_dev != value["device"] or m.os_mount() is None:
        raise RuntimeError("Cancel replaced or removed the original kernel volume")
    data = (r.R / "seeds" / KEY).read_bytes()
    read = m.check_bytes(KEY, data)
    m.emit("native-force-cancel-observed", same_device=True, writer_still_held=True,
           read=read, mounts=view, entries=r.entries())
    return {"cancel_retained_mount": True, "exact_bytes": read["matches"]}


def force_check():
    value = ready()
    if value["released"] or r._identity(value["holder_pid"]) != value["holder_identity"]:
        raise RuntimeError("Confirm must be checked while the original writer remains held")
    # The UI itself requested the explicit daemon force operation. Observe its
    # result; this helper sends no CLI/API eject or reconciliation request.
    m.wait_automatically_forgotten("native-confirmed-force-observation")
    entry = m.wait_queued(KEY, value["bytes"])
    result = {"key": KEY, "entry_id": entry["id"], "bytes": value["bytes"],
              "old_device": value["device"], "holder_still_open": True}
    r._atomic_json(CONFIRMED, result)
    m.emit("native-force-confirm-observed", **result, mounts=m.mounts())
    return result


def recover():
    value = ready()
    if not value["released"] or not CONFIRMED.exists():
        raise RuntimeError("observe confirmed force, release the writer and wait for its exit first")
    data = (r.R / "seeds" / KEY).read_bytes()
    if hashlib.sha256(data).hexdigest() != value["sha256"]:
        raise RuntimeError("private source payload changed")
    m.wait_automatically_forgotten("native-force-recovery-observation")
    m.restart_no_remount("native-confirmed-force", KEY, len(data))
    m.remount_restore_and_publish("native-confirmed-force", KEY, data, value["device"])
    m.emit("native-force-exact-byte-recovery-passed", mounts=m.mounts(), entries=r.entries())
    return {"native_force_recovery": "passed"}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["hold", "cancel-check", "force-check", "recover"])
    args = parser.parse_args()
    result = globals()[args.command.replace("-", "_")]()
    if result is not None:
        print(json.dumps(r.sanitized(result), sort_keys=True))
