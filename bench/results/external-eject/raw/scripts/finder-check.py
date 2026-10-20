#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Hold a private offline save for an operator's native Finder eject check.

hold keeps the writer open until the operator touches the rig's finder-release
file. The operator can first verify Finder's normal busy refusal, release the
writer, and then eject normally. recover observes automatic daemon cleanup;
it never calls CLI unmount to reconcile the Finder eject.
"""

import argparse
import importlib.util
import json
import os
from pathlib import Path

import rig as r


def matrix():
    spec = importlib.util.spec_from_file_location("external_matrix", Path(__file__).with_name("live-matrix.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def hold(m):
    r.verify_rig()
    m.ensure_mounted()
    r.offline()
    ready, release = r.R / "finder-ready.json", r.R / "finder-release"
    if ready.exists():
        raise RuntimeError("use a fresh rig for a new Finder measurement")
    release.unlink(missing_ok=True)
    descriptor, data = m.write_open("finder-external-offline.bin", 98357)
    value = {"device": os.stat(r.MNT).st_dev, "key": "finder-external-offline.bin",
             "bytes": len(data), "holder_pid": os.getpid(), "released": False}
    r._atomic_json(ready, value)
    m.emit("finder-holder-ready", **value)
    try:
        r.wait(release.exists, timeout=300, what="operator releases the Finder test writer")
    finally:
        try:
            os.close(descriptor)
        except OSError as error:
            m.emit("finder-holder-close-error", errno=error.errno)
        value["released"] = True
        r._atomic_json(ready, value)
        m.emit("finder-holder-closed", **value)
    m.wait_queued(value["key"], len(data))


def recover(m):
    r.verify_rig()
    value = json.loads((r.R / "finder-ready.json").read_text())
    if not value.get("released"):
        raise RuntimeError("release and close the writer before normal Finder eject recovery")
    if m.os_mount() is not None:
        raise RuntimeError("Finder has not ejected the disposable volume")
    data = (r.R / "seeds" / value["key"]).read_bytes()
    # The operator's GUI action is not timestamped here. This measures observation
    # only; it cannot establish an eject-to-reconciliation latency.
    m.wait_automatically_forgotten("native-Finder-observation")
    m.restart_no_remount("native-Finder", value["key"], len(data))
    m.remount_restore_and_publish("native-Finder", value["key"], data, value["device"])
    m.emit("native-Finder-external-eject-recovery-passed", mounts=m.mounts(), entries=r.entries())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["hold", "recover"])
    args = parser.parse_args()
    module = matrix()
    globals()[args.command](module)


if __name__ == "__main__":
    main()
