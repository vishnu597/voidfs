#!/usr/bin/python3
# SPDX-License-Identifier: Apache-2.0
"""Read-only cache observations around the first native open of a fresh drive fork.

This never mounts, opens media, hashes mounted files, changes daemon state, or
clears caches. A fresh fork ID and fresh read-only mount supply cold client/kernel
identity; --expect-cold verifies the fork has no indexed body blocks or stages.
Counters below are observed cache occupancy, not cumulative network bytes.
"""

import argparse
import datetime
import json
from pathlib import Path
import re
import sqlite3
import subprocess
import time


def command(argv):
    result = subprocess.run([str(value) for value in argv], capture_output=True, text=True, timeout=30)
    if result.returncode:
        raise RuntimeError("command failed: " + " ".join(map(str, argv)) + ": " + result.stderr.strip())
    return result.stdout


def snapshot(args):
    database = args.state_dir.resolve() / "state.sqlite"
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True, timeout=10) as connection:
        connection.execute("PRAGMA query_only=ON")
        connection.execute("BEGIN")
        total = connection.execute("SELECT COUNT(*), COALESCE(SUM(len),0) FROM cache_blocks").fetchone()
        rows = connection.execute("SELECT drive, COUNT(*), SUM(len) FROM cache_blocks WHERE drive IN (?,?) GROUP BY drive",
                                  (args.drive, args.drive_alias or args.drive)).fetchall()
        stages = connection.execute("SELECT COUNT(*) FROM mount_staged s JOIN mount_inodes i ON i.ino=s.ino WHERE i.drive IN (?,?)",
                                    (args.drive, args.drive_alias or args.drive)).fetchone()[0]
        entries = connection.execute("SELECT COUNT(*) FROM entries WHERE drive IN (?,?)", (args.drive, args.drive_alias or args.drive)).fetchone()[0]
    status = json.loads(command([args.void, "--json", "status"]))
    allocated = {}
    paths = [args.state_dir.resolve() / name for name in ["cache", "mount-stage", "journal"]]
    paths = [path for path in paths if path.exists()]
    if paths:
        for line in command(["/usr/bin/du", "-sk", *paths]).splitlines():
            count, path = line.split("\t", 1)
            allocated[Path(path).name] = int(count) * 1024
    mount = args.mount.resolve()
    mount_line = next((line for line in command(["/sbin/mount"]).splitlines() if " on " + str(mount) + " (" in line), None)
    resources = {"voidfs://" + args.drive}
    if args.drive_alias:
        resources.add("voidfs://" + args.drive_alias)
    fork_mount = bool(mount_line and mount_line.split(" on ", 1)[0] in resources and " (voidfs," in mount_line)
    return {"kind": "cache_snapshot", "label": args.label, "utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
            "drive_id": args.drive, "drive_alias": args.drive_alias, "mount": str(mount), "mount_table_line": mount_line,
            "fork_resource_matches": fork_mount, "read_only_mount": bool(fork_mount and "read-only" in mount_line),
            "fork_cache_blocks": sum(row[1] for row in rows), "fork_cache_bytes": sum(row[2] for row in rows),
            "fork_cache_by_name": [{"drive": name, "blocks": count, "bytes": size} for name, count, size in rows],
            "fork_stages": stages, "fork_journal_entries": entries, "global_cache_blocks": total[0], "global_cache_bytes": total[1],
            "daemon_cache": status["cache"], "allocated_bytes": allocated,
            "scope": "client indexed cache occupancy and fresh read-only kernel mount; server/object-store caches may be warm"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", type=Path, required=True)
    parser.add_argument("--drive", required=True, help="fresh fork's stable d-UUID ID, never an unresolved alias")
    parser.add_argument("--drive-alias", help="also verify no blocks/stages exist under the fork alias")
    parser.add_argument("--mount", type=Path, required=True)
    parser.add_argument("--void", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--label", required=True)
    parser.add_argument("--expect-cold", action="store_true", help="assert zero fork cache, stages and journal entries on the first observation")
    parser.add_argument("--seconds", type=float, default=0, help="observe cache growth for this duration; zero makes one snapshot")
    parser.add_argument("--interval", type=float, default=2)
    parser.add_argument("--maximum-cache-bytes", type=int, default=8000000000, help="bound combined allocated cache use in this isolated rig")
    args = parser.parse_args()
    if not re.fullmatch(r"d-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", args.drive):
        parser.error("--drive must be the resolved fresh stable drive ID")
    if args.seconds < 0 or args.interval <= 0 or args.maximum_cache_bytes < 1:
        parser.error("duration, interval and cache budget must be valid")
    args.out.parent.mkdir(parents=True, exist_ok=True)
    samples, failures, started = [], [], time.monotonic()
    with args.out.open("x") as output:
        def emit(value):
            output.write(json.dumps(value, ensure_ascii=False) + "\n")
            output.flush()

        try:
            while True:
                value = snapshot(args)
                emit(value)
                samples.append(value)
                if not value["read_only_mount"]:
                    failures.append("fresh native mount is absent or not read-only")
                if len(samples) == 1 and args.expect_cold and any(value[field] != 0 for field in ["fork_cache_blocks", "fork_cache_bytes", "fork_stages", "fork_journal_entries"]):
                    failures.append("fork was not cold before native body reads")
                if value["allocated_bytes"].get("cache", 0) > args.maximum_cache_bytes:
                    failures.append("combined allocated cache exceeded the isolated rig budget")
                if failures or time.monotonic() - started >= args.seconds:
                    break
                time.sleep(min(args.interval, max(0, args.seconds - (time.monotonic() - started))))
        except KeyboardInterrupt:
            pass
        except (OSError, ValueError, RuntimeError, sqlite3.Error, subprocess.SubprocessError) as error:
            failures.append(str(error))
        summary = {"kind": "summary", "label": args.label, "samples": len(samples), "elapsed_seconds": round(time.monotonic() - started, 3),
                   "ok": bool(samples) and not failures, "failures": sorted(set(failures)), "maximum_cache_bytes": args.maximum_cache_bytes,
                   "peak_fork_cache_bytes": max((value["fork_cache_bytes"] for value in samples), default=0),
                   "peak_global_cache_bytes": max((value["global_cache_bytes"] for value in samples), default=0),
                   "peak_allocated_cache_bytes": max((value["allocated_bytes"].get("cache", 0) for value in samples), default=0),
                   "peak_daemon_memory_cache_bytes": max((value["daemon_cache"]["memoryBytes"] for value in samples), default=0),
                   "network_fetch_bytes_measured": False, "mounted_body_read_by_probe": False}
        emit(summary)
    print(json.dumps(summary))
    return 0 if summary["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
