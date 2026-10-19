#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Operator-run FSKit feed race on the disposable notarized-update rig.

Importing this file does nothing. Run explicitly after the primary update checks:
  /usr/bin/python3 feed-race.py --run

The test restarts the rig daemon offline, deliberately primes two negative lookups,
and makes the daemon learn remote creations before releasing their feed events.
It requires both Object invalidations on a read-only RPC watch. FSKit's resulting
listings and path reads are recorded separately: its known pinned-miss limitation
is an observation, not an assertion that a deliberately missed path must read.
Only the existing disposable rig is controlled; credentials are never printed.
"""

import argparse
import errno
import hashlib
import json
import os
from pathlib import Path
import queue
import threading
import time
import uuid

import rig


MAX_LINE = 128 * 1024
PROBE = r"""
import errno, hashlib, json, os, sys
operation, paths = sys.argv[1], json.loads(sys.argv[2])
rows = []
for path in paths:
    try:
        if operation == "list":
            row = {"names": sorted(os.listdir(path))}
        elif operation == "stat":
            value = os.stat(path)
            row = {"size": value.st_size, "mode": value.st_mode}
        elif operation == "read":
            with open(path, "rb") as stream:
                value = stream.read(1024 * 1024 + 1)
            if len(value) > 1024 * 1024:
                raise ValueError("probe read exceeds its one MiB limit")
            row = {"size": len(value), "sha256": hashlib.sha256(value).hexdigest()}
        else:
            raise ValueError("unknown probe operation")
        rows.append({"path": path, "ok": True, **row})
    except OSError as error:
        rows.append({"path": path, "ok": False, "errno": error.errno})
print(json.dumps(rows, sort_keys=True))
"""


def probe(operation, paths, timeout=15):
    """Every mounted filesystem operation has an external process timeout."""
    result = rig.run(["/usr/bin/python3", "-c", PROBE, operation,
                      json.dumps([str(path) for path in paths])], timeout=timeout)
    return json.loads(result.stdout)


def request(method, path, body=None, generation=None, timeout=10):
    connection = rig.UnixHTTPConnection(rig.D / "daemon.sock", timeout=timeout)
    headers = {"Content-Type": "application/json"}
    if generation is not None:
        headers["x-voidfs-generation"] = str(generation)
    try:
        payload = None if body is None else json.dumps(body).encode("utf-8")
        connection.request(method, path, body=payload, headers=headers)
        response = connection.getresponse()
        raw = response.read(MAX_LINE + 1)
        if len(raw) > MAX_LINE:
            raise RuntimeError("RPC response exceeded the test's bound")
        value = json.loads(raw) if raw else None
        if not 200 <= response.status < 300:
            raise RuntimeError("RPC HTTP %s: %s" %
                               (response.status, rig.sanitized(value)))
        return value
    finally:
        connection.close()


class Reader:
    """Session, calls and watch share this controller's PID, as RPC requires."""
    def __init__(self):
        self.info = request("POST", "/v1/fs/sessions",
                            {"version": 1, "drive": rig.drive, "readOnly": True})
        self.path = "/v1/fs/" + self.info["id"]
        self.generation = self.info["generation"]
        self.connection = None
        self.thread = None
        self.events = queue.Queue()

    def call(self, operation, body):
        return request("POST", self.path + "/" + operation, body,
                       generation=self.generation)

    def listing(self, ino):
        entries, after, generation = [], None, None
        for _ in range(64):
            page = self.call("readdir", {"ino": ino, "after": after, "limit": 256})
            if generation is not None and page["generation"] != generation:
                raise RuntimeError("RPC listing changed generation while feed was gated")
            generation = page["generation"]
            rows = page["entries"]
            entries.extend(rows)
            if len(rows) < 256:
                return entries
            cursor = rows[-1][0]
            if after is not None and cursor <= after:
                raise RuntimeError("RPC listing cursor did not advance")
            after = cursor
        raise RuntimeError("RPC listing exceeded 64 pages")

    def watch(self):
        connection = rig.UnixHTTPConnection(rig.D / "daemon.sock", timeout=60)
        self.connection = connection
        connection.request("GET", self.path + "/watch",
                           headers={"x-voidfs-generation": str(self.generation)})
        response = connection.getresponse()
        if response.status != 200:
            raise RuntimeError("RPC watch HTTP %s" % response.status)

        def consume():
            try:
                while True:
                    line = response.readline(MAX_LINE + 1)
                    if not line:
                        raise RuntimeError("RPC watch ended before the test closed it")
                    if len(line) > MAX_LINE or not line.endswith(b"\n"):
                        raise RuntimeError("RPC watch line exceeded its bound")
                    self.events.put(("event", json.loads(line)))
            except Exception as error:
                self.events.put(("error", rig.sanitized(str(error))))

        self.thread = threading.Thread(target=consume, daemon=True)
        self.thread.start()
        initial = self.next_event(timeout=10)
        if not initial.get("resync"):
            raise RuntimeError("RPC watch did not begin with its initial resync")
        return initial

    def next_event(self, timeout):
        try:
            kind, value = self.events.get(timeout=timeout)
        except queue.Empty:
            raise TimeoutError("timed out waiting for the RPC invalidation watch") from None
        if kind == "error":
            raise RuntimeError(value)
        return value

    def close(self):
        try:
            self.call("release", {})
        finally:
            if self.connection is not None:
                self.connection.close()
            if self.thread is not None:
                self.thread.join(timeout=1)


def status():
    return rig.ctl("GET", "/v1/status", timeout=5)


def mounted():
    view = rig.ctl("GET", "/v1/mounts", timeout=5)
    return next((mount for mount in view["mounts"]
                 if mount["drive"] == rig.drive
                 and Path(mount["mountpoint"]) == rig.MNT
                 and mount["state"] == "mounted"), None)


def wait_link(wanted):
    def matches():
        value = status()
        return value if value["connection"]["link"] == wanted else None
    return rig.wait(matches, timeout=60, interval=0.2, what="rig link " + wanted)


def main(output):
    started = time.monotonic()
    report = {"controllerPid": os.getpid(), "events": [], "passed": False}
    reader = None
    recover_proxy = False

    def record(event, **data):
        value = rig.sanitized({"event": event, "elapsed": time.monotonic() - started,
                               **data})
        report["events"].append(value)
        rig.emit("feed-race-" + event, **data)

    try:
        previous = json.loads(rig.MODE.read_text(encoding="utf-8"))
        if previous.get("hold_feed") or previous.get("fail_mutations"):
            raise RuntimeError("finish the primary rig checks and clear proxy fault modes first")
        if not mounted():
            raise RuntimeError("the disposable rig's expected mount is not mounted")
        if status()["connection"]["link"] != "online":
            raise RuntimeError("the disposable rig must begin online")

        # Cache both baseline directories and bytes before the offline restart. These bounded
        # children exit before restart, leaving no descriptor that could make eject busy.
        baseline = probe("read", [rig.MNT / "baseline-root.txt",
                                  rig.MNT / "nested/baseline-nested.txt"])
        if not all(row["ok"] for row in baseline):
            raise RuntimeError("the rig's root and nested baseline must read before the race")
        record("baseline", mounted=mounted(), reads=baseline)
        token = uuid.uuid4().hex[:12]
        root_name, child_name = "feed-race-root-" + token + ".txt", "feed-race-child-" + token + ".txt"
        keys = [root_name, "nested/" + child_name]
        paths = [rig.MNT / key for key in keys]
        report["keys"] = keys

        recover_proxy = True
        rig.stop_proxy()
        wait_link("offline")
        before_pid = status()["daemon"]["pid"]
        restarted = rig.run([rig.VOID, "--json", "daemon", "restart"], timeout=90)
        rig.wait(mounted, timeout=60, interval=0.2, what="remembered rig mount restoration")
        offline = wait_link("offline")
        if offline["daemon"]["pid"] == before_pid:
            raise RuntimeError("daemon restart retained the old PID")
        record("restored-offline", beforePid=before_pid,
               afterPid=offline["daemon"]["pid"], restartStdout=restarted.stdout,
               mount=mounted())

        offline_reads = probe("read", [rig.MNT / "baseline-root.txt",
                                       rig.MNT / "nested/baseline-nested.txt"])
        if not all(row["ok"] and row.get("sha256") == original.get("sha256")
                   and row.get("size") == original.get("size")
                   for row, original in zip(offline_reads, baseline)):
            raise RuntimeError("restored baseline bytes did not match their cached offline copies")
        negatives = probe("stat", paths)
        if not all(not row["ok"] and row.get("errno") == errno.ENOENT
                   for row in negatives):
            raise RuntimeError("offline negative-cache priming did not return ENOENT")
        if status()["connection"]["link"] != "offline":
            raise RuntimeError("negative lookups were not primed entirely offline")
        record("primed-offline-misses", reads=offline_reads, lookups=negatives)

        rig.mode(hold_feed=True)
        rig.start_proxy()
        online = wait_link("online")
        record("online-with-feed-held", connection=online["connection"], mounts=online["mounts"])
        expected = {}
        for key in keys:
            content = ("controlled feed race: " + key + "\n").encode("utf-8")
            source = rig.R / "seeds" / Path(key).name
            source.write_bytes(content)
            target = rig.drive + (":/nested/" if key.startswith("nested/") else ":/")
            uploaded = rig.peer("upload", source, target, timeout=60)
            expected[key] = {"size": len(content), "sha256": hashlib.sha256(content).hexdigest()}
            record("peer-created", key=key, expected=expected[key], stdout=uploaded.stdout)

        # Existing mount ownership keeps this RPC session on exactly the restored drive core.
        # This Python PID stays alive for creation, watch, readdir and explicit release.
        reader = Reader()
        initial = reader.watch()
        record("rpc-watch-initial", session={"root": reader.info["root"],
                                              "generation": reader.generation,
                                              "readOnly": reader.info["readOnly"]}, invalidation=initial)
        root_entries = reader.listing(reader.info["root"])
        nested = next((attr["ino"] for name, attr in root_entries if name == "nested"), None)
        if nested is None:
            raise RuntimeError("refreshed root has no baseline nested directory")
        nested_entries = reader.listing(nested)
        root_names = [name for name, _ in root_entries]
        nested_names = [name for name, _ in nested_entries]
        if root_name not in root_names or child_name not in nested_names:
            raise RuntimeError("the daemon did not list both remote names before feed release")
        record("rpc-listed-before-feed", rootNames=root_names, nestedNames=nested_names)
        while not reader.events.empty():
            early = reader.next_event(timeout=1)
            if any(item.get("kind") == "object" and item.get("key") in keys
                   for item in early.get("invalidations", [])):
                raise RuntimeError("a target feed event arrived while its body was gated")
            record("rpc-event-before-release", invalidation=early)

        rig.mode()
        record("feed-released")
        received = set()
        deadline = time.monotonic() + 45
        while received != set(keys):
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("both foreign Object invalidations did not arrive")
            event = reader.next_event(timeout=remaining)
            record("rpc-event-after-release", invalidation=event)
            if not event.get("local", False) and event["generation"] > initial["generation"]:
                received.update(item["key"] for item in event.get("invalidations", [])
                                if item.get("kind") == "object" and item.get("key") in keys)
        record("required-invalidations-received", keys=sorted(received))

        # Poll only listings before inspecting the two deliberately missed names again. A
        # pinned root miss is retained as an Apple-limitation observation, not a core failure.
        listings, attempts = [], 0
        listing_deadline = time.monotonic() + 10
        while True:
            attempts += 1
            listings = probe("list", [rig.MNT, rig.MNT / "nested"], timeout=8)
            listed = (all(row["ok"] for row in listings)
                      and root_name in listings[0].get("names", [])
                      and child_name in listings[1].get("names", []))
            if listed or time.monotonic() >= listing_deadline:
                break
            time.sleep(0.2)
        lookups = probe("stat", paths)
        reads = probe("read", paths)
        for key, row in zip(keys, reads):
            row["matchesPeerBytes"] = (row["ok"] and row.get("size") == expected[key]["size"]
                                        and row.get("sha256") == expected[key]["sha256"])
        report["fskit"] = {"bothListed": listed, "listingAttempts": attempts,
                            "listings": listings, "lookups": lookups, "reads": reads}
        record("fskit-observed", **report["fskit"])
        report["passed"] = True
    except Exception as error:
        report["error"] = rig.sanitized(str(error))
        record("failed", reason=report["error"])
    finally:
        if recover_proxy:
            try:
                rig.mode()
                rig.start_proxy()
            except Exception as error:
                report["cleanupError"] = rig.sanitized(str(error))
                report["passed"] = False
        if reader is not None:
            try:
                reader.close()
            except Exception as error:
                report["sessionReleaseError"] = rig.sanitized(str(error))
                report["passed"] = False
        report["elapsed"] = time.monotonic() - started
        output.write_text(json.dumps(rig.sanitized(report), indent=2, sort_keys=True) + "\n",
                          encoding="utf-8")
        print(json.dumps({"passed": report["passed"], "report": str(output),
                          "error": report.get("error")}, sort_keys=True))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run", action="store_true", help="explicitly operate the disposable rig")
    parser.add_argument("--output", type=Path, default=rig.evidence / "feed-race.json")
    arguments = parser.parse_args()
    if not arguments.run:
        parser.error("pass --run only after the operator has finished the primary rig checks")
    raise SystemExit(main(arguments.output))
