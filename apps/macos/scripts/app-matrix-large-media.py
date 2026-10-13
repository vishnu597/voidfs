#!/usr/bin/python3
# SPDX-License-Identifier: Apache-2.0
"""Generate unique real ProRes footage directly on a test mount and reconcile its bytes.

Build app-matrix-media.swift first. Cases are fresh by default. To continue a verified case,
repeat --resume-evidence for every earlier JSONL run; existing media is never rewritten.
Small control: --target-bytes 170000000 --frames-per-clip 300 --case media-small.
Large project: --target-bytes 50000000000 --frames-per-clip 3000 --case media-50gb.

Source SHA-256 is read while the test drive's uploads are paused, from unpublished staging.
After publication, SHA-256 streams from the server without a full mounted reread. First and
last clips receive native AVFoundation random scrubs, then warm repeats; a two-second HEVC
export is also read and reconciled. File/frame/seed/hash and disk observations go into JSONL
and a small manifest in the case folder. The caller owns mount setup and eventual cleanup.
"""

import argparse
import datetime
import fcntl
import importlib.util
import ipaddress
import json
import math
import os
from pathlib import Path
import re
import shutil
import subprocess
import threading
import time
from urllib.parse import urlparse


spec = importlib.util.spec_from_file_location("app_matrix", Path(__file__).with_name("app-matrix.py"))
common = importlib.util.module_from_spec(spec)
spec.loader.exec_module(common)


class DiskSamples:
    def __init__(self, args, case, run=None, label=None):
        self.run, self.label = run, label
        self.paths = {"source_view": case}
        if args.drive:
            self.paths.update({"mount_stage": Path(args.state_dir) / "mount-stage",
                               "journal": Path(args.state_dir) / "journal",
                               "cache": Path(args.state_dir) / "cache", "server": Path(args.server_dir)})
        self.samples = []
        self.stop = threading.Event()
        self.worker = threading.Thread(target=self.poll, daemon=True)

    def once(self):
        present = [(name, path) for name, path in self.paths.items() if path.exists()]
        if not present:
            return
        try:
            attempts = []
            for attempt in range(3):
                result = subprocess.run(["/usr/bin/du", "-sk", *[str(path) for name, path in present]],
                                        capture_output=True, text=True, timeout=15)
                if result.returncode == 0:
                    break
                attempts.append({"rc": result.returncode, "stdout": result.stdout, "stderr": result.stderr})
                # Publication deletes/replaces temporary files while du walks the tree. Retry
                # only that explicit race; permissions and other failures remain failures.
                errors = result.stderr.splitlines()
                if not errors or not all(line.startswith("du: ") and line.endswith(": No such file or directory") for line in errors):
                    break
                time.sleep(0.1)
            sizes = {}
            for line in result.stdout.splitlines():
                size, path = line.split("\t", 1)
                sizes[path] = int(size) * 1024
            self.samples.append({"unix": time.time(), "allocated_bytes": {name: sizes.get(str(path)) for name, path in present},
                                 "rc": result.returncode, "stderr": result.stderr, "retry_attempts": attempts})
        except (OSError, ValueError, subprocess.TimeoutExpired) as error:
            self.samples.append({"unix": time.time(), "error": str(error)})

    def poll(self):
        while not self.stop.is_set():
            self.once()
            self.stop.wait(2)

    def __enter__(self):
        self.worker.start()
        return self

    def __exit__(self, *args):
        self.stop.set()
        self.worker.join(20)
        self.once()
        if args[0] is not None and self.run:
            self.record(self.run, self.label + " failed")

    def record(self, run, label):
        peaks = {name: max((s.get("allocated_bytes", {}).get(name) or 0 for s in self.samples), default=0)
                 for name in self.paths}
        for sample in self.samples:
            run.emit("disk_sample", label=label, **sample)
        run.emit("disk_peak", label=label, allocated_bytes=peaks,
                 source_view_is_virtual=bool(run.args.drive), apfs_source_copy_bytes=0 if run.args.drive else peaks["source_view"])
        run.check(label + ": disk samples valid", bool(self.samples) and all(s.get("rc") == 0 for s in self.samples))
        return peaks


def parsed(run, name, argv, **kwargs):
    rc, out, err = run.command(name, argv, **kwargs)
    if rc:
        raise RuntimeError(name + " failed")
    try:
        result = json.loads(out)
    except ValueError as error:
        run.check(name + ": JSON result", False, stdout=out, error=str(error))
        raise
    run.emit("media_result", name=name, result=result)
    return result


def assert_inspection(run, name, result, duration, width, height):
    run.check(name + ": duration", abs(result["durationSeconds"] - duration) <= 1 / 30,
              expected_seconds=duration, actual_seconds=result["durationSeconds"])
    run.check(name + ": dimensions", result["width"] == width and result["height"] == height,
              expected=[width, height], actual=[result["width"], result["height"]])
    scrubs = result.get("scrubs", [])
    run.check(name + ": native random scrubs decode frames", len(scrubs) == 6 and all(s.get("decodedFrames", 0) > 0 for s in scrubs),
              scrubs=scrubs)


def inspect_clip(run, args, clip):
    path = Path(clip["path"])
    fd = os.open(path, os.O_RDONLY)
    try:
        fcntl.fcntl(fd, 48, 1)  # Darwin F_NOCACHE; inherited descriptor bypasses kernel data cache.
        rc, out, err = run.command("attempt native scrubs with kernel-cache bypass " + path.name,
                                  [args.media, "inspect", "/dev/fd/" + str(fd)], pass_fds=(fd,), expected=(0, 1))
    finally:
        os.close(fd)
    if rc == 0:
        result = json.loads(out)
        run.emit("media_result", name="cold native scrubs " + path.name, result=result)
        run.emit("read_method", path=str(path), kernel_cache_bypass=True,
                 client_cache="first decoder read of newly published content; no mounted full reread after publication")
        assert_inspection(run, "cold " + path.name, result, clip["frames"] / 30, args.width, args.height)
    else:
        run.limit("cold native scrubs untested: AVFoundation descriptor URL unsupported on APFS control; fresh independent mount needed",
                  path=str(path), stderr=err)
        result = parsed(run, "first native scrubs " + path.name, [args.media, "inspect", path])
        assert_inspection(run, "first " + path.name, result, clip["frames"] / 30, args.width, args.height)
    warm = parsed(run, "warm native scrubs " + path.name, [args.media, "inspect", path])
    assert_inspection(run, "warm " + path.name, warm, clip["frames"] / 30, args.width, args.height)


def verify_metadata(run, path, source):
    if not run.args.drive:
        return
    key = run.key(path)
    versions = run.history(path)
    run.emit("versions", key=key, versions=versions)
    run.check("media has a current server version", bool(versions) and any(v.get("isLatest") for v in versions), key=key)
    attrs = parsed(run, "media server metadata", [run.args.metadata, run.args.drive, "attrs", key])
    if attrs.get("mode") is None:
        run.limit("media server mode omitted; comparison untested", key=key)
    else:
        run.check("media source/server mode", int(attrs["mode"], 8) == int(source["mode"], 8), key=key)
    if attrs.get("mtime") is None:
        run.limit("media server mtime omitted; comparison untested", key=key)
    else:
        actual = datetime.datetime.fromisoformat(attrs["mtime"].replace("Z", "+00:00")).timestamp()
        run.check("media source/server mtime", abs(actual - source["mtime_ns"] / 1e9) < 0.001, key=key)
    xattrs = common.expected_server_xattrs(attrs, run, key)
    run.check("media source/server xattrs", xattrs == source["xattrs"], key=key, source=source["xattrs"], server=xattrs)


def paused_source(run, args, path, name, command, json_result=True):
    paused = False
    try:
        if args.drive:
            rc, out, err = run.command(name + ": pause test-drive uploads by alias", [run.void, "uploads", "pause", "--drive", args.drive])
            if rc:
                raise RuntimeError("cannot pause test drive")
            paused = True
            status = parsed(run, name + ": verify pause", [run.void, "--json", "status"])
            if not run.check("alias pause includes mounted drive ID before source generation", run.drive_id in status["uploads"]["pausedDrives"],
                             requested_alias=args.drive, mounted_drive_id=run.drive_id, paused_drives=status["uploads"]["pausedDrives"]):
                raise RuntimeError("test drive pause not applied")
        if json_result:
            result = parsed(run, name, command)
        else:
            rc, out, err = run.command(name, command)
            if rc:
                raise RuntimeError(name + " failed")
            result = {"stdout": out, "stderr": err}
        # This is the only full mounted read. Pausing keeps its source in local staging.
        source = common.metadata(path)
        run.emit("source_file", unpublished_staging=bool(args.drive), **source)
        return result, source
    finally:
        if paused:
            rc, out, err = run.command(name + ": resume test-drive uploads by alias", [run.void, "uploads", "resume", "--drive", args.drive])
            if rc:
                raise RuntimeError("cannot restore test-drive uploads")


def publish(run, path, source):
    if not run.args.drive:
        return source["sha256"]
    if not run.drain("media publication"):
        raise RuntimeError("media publication did not drain")
    remote = run.server_hash(run.key(path))
    if not run.check("unpublished source and published server SHA-256 agree", remote["rc"] == 0 and
                     remote["sha256"] == source["sha256"] and remote["size"] == source["size"],
                     source=source, server=remote):
        raise RuntimeError("published media bytes differ")
    verify_metadata(run, path, source)
    return remote["sha256"]


def media_row(run, args):
    case = run.root / args.case
    if case.exists() and not args.resume_evidence:
        raise RuntimeError("media case already exists; use a fresh case name")
    if args.drive:
        status = parsed(run, "media preflight daemon", [run.void, "--json", "status"])
        host = urlparse(status["daemon"]["endpoint"]).hostname
        loopback = host == "localhost" or (host is not None and ipaddress.ip_address(host).is_loopback)
        expected_state = str(Path(args.state_dir).resolve())
        if not run.check("media uses isolated local test daemon", loopback and status["daemon"]["stateDir"] == expected_state,
                         endpoint=status["daemon"]["endpoint"], state_dir=status["daemon"]["stateDir"]):
            raise RuntimeError("media requires an isolated loopback test daemon")
        if status["uploads"]["paused"] or run.drive_id in status["uploads"]["pausedDrives"] or args.drive in status["uploads"]["pausedDrives"]:
            raise RuntimeError("test drive was already paused; preserve its existing pause")
    planned = 3 * (args.target_bytes + args.max_overshoot_bytes) + args.read_budget_bytes + args.other_budget_bytes
    if not run.check("planned cumulative write budget below cap", planned < args.write_budget_bytes,
                     model="stage + frozen journal + fs server; decoded/cache allowance; prior rig/apps allowance",
                     planned_bytes=planned, cap_bytes=args.write_budget_bytes):
        raise RuntimeError("planned disk writes exceed budget")
    free = shutil.disk_usage(args.state_dir if args.drive else run.root).free
    run.emit("media_plan", target_encoded_bytes=args.target_bytes, max_overshoot_bytes=args.max_overshoot_bytes,
             frames_per_clip=args.frames_per_clip, dimensions=[args.width, args.height], codec="ProRes422", fps=30,
             seed=args.seed, inspect_clips="first and last", mounted_source_reads="once while paused; none full after publication",
             planned_cumulative_bytes=planned, free_bytes=free, source_copy_bytes=0 if args.drive else None)
    if not run.check("disk has room for project and staging", free > args.target_bytes + 6_000_000_000 + args.read_budget_bytes):
        raise RuntimeError("insufficient disk space")
    case.mkdir(exist_ok=bool(args.resume_evidence))
    run.scope = case
    manifest = {"case": args.case, "targetBytes": args.target_bytes, "clips": [], "encodedBytes": 0, "projectKind": "unique real ProRes422 frames"}
    initial = DiskSamples(args, case)
    initial.once()
    baseline_cache = initial.samples[-1].get("allocated_bytes", {}).get("cache", 0) or 0
    bytes_per_frame = args.estimated_bytes_per_frame
    last_inspected = None
    if args.resume_evidence:
        manifest = json.loads((case / "manifest.json").read_text())
        prior = [json.loads(line) for evidence in args.resume_evidence for line in Path(evidence).open()]
        prior_clips = {record["path"]: record for record in prior if record["kind"] == "media_clip"}
        prior_versions = {record["key"]: record["versions"] for record in prior if record["kind"] == "versions"}
        if not run.check("resume manifest matches recorded verified clips", manifest["case"] == args.case and
                         manifest["targetBytes"] == args.target_bytes and manifest["encodedBytes"] == sum(c["bytes"] for c in manifest["clips"]) and
                         bool(manifest["clips"]) and all(c["path"] in prior_clips and
                         all(c[field] == prior_clips[c["path"]][field] for field in ("bytes", "frames", "seed", "sourceSha256", "serverSha256")) and
                         c["sourceSha256"] == c["serverSha256"] for c in manifest["clips"]), evidence=args.resume_evidence):
            raise RuntimeError("resume requires complete prior publication evidence")
        for number, clip in enumerate(manifest["clips"], 1):
            path = case / ("clip-%04d.mov" % number)
            if str(path) != clip["path"] or not path.is_file():
                raise RuntimeError("resume clip paths do not match the case")
            key = run.key(path)
            expected = next((v["versionId"] for v in prior_versions.get(key, []) if v.get("isLatest")), None)
            current = run.history(path)
            actual = next((v["versionId"] for v in current if v.get("isLatest")), None)
            if not expected or not actual:
                raise RuntimeError("resume requires a recorded and current server version")
            if expected == actual:
                run.check("resume previously reconciled server version is unchanged", True,
                          key=key, expected=expected, actual=actual, retained_sha256=clip["serverSha256"])
            else:
                run.emit("resume_version_changed", key=key, expected=expected, actual=actual,
                         action="reconcile current server bytes and metadata; no full mounted reread")
                remote = run.server_hash(key)
                if not run.check("resume changed version retains original media SHA", remote["rc"] == 0 and
                                 remote["sha256"] == clip["sourceSha256"] and remote["size"] == clip["bytes"], key=key, remote=remote):
                    raise RuntimeError("original media bytes changed before resume")
                current_source = common.metadata(path, content=False)
                run.emit("resume_current_metadata", content_hash_omitted=True, retained_source_sha256=clip["sourceSha256"], **current_source)
                verify_metadata(run, path, current_source)
                if run.failures:
                    raise RuntimeError("changed media metadata did not reconcile before resume")
        cache_samples = [r.get("allocated_bytes", {}).get("cache") for r in prior if r["kind"] == "disk_sample"]
        baseline_cache = min(value for value in cache_samples if value is not None)
        bytes_per_frame = manifest["clips"][-1]["bytes"] / manifest["clips"][-1]["frames"]
        last_inspected = 1  # The first clip's native checks are retained in the original evidence.
        run.emit("media_resume", evidence=args.resume_evidence, retained_clips=len(manifest["clips"]),
                 retained_encoded_bytes=manifest["encodedBytes"], original_cache_baseline_bytes=baseline_cache,
                 mounted_full_rereads=0, reason="continue after disk sampling caught temporary-file deletion")
    while manifest["encodedBytes"] < args.target_bytes:
        remaining = args.target_bytes - manifest["encodedBytes"]
        frames = args.frames_per_clip if not bytes_per_frame else min(args.frames_per_clip, max(60, math.ceil(remaining / bytes_per_frame)))
        number = len(manifest["clips"]) + 1
        path = case / ("clip-%04d.mov" % number)
        seed = (args.seed + number * 2654435761) % (1 << 32)
        with DiskSamples(args, case, run, path.name) as disk:
            result, source = paused_source(run, args, path, "make " + path.name,
                                           [args.media, "make", path, frames, args.width, args.height, seed])
            if not run.check("generated clip is real expected frame sequence", result["frames"] == frames and
                             result["bytes"] == source["size"] and result["codec"] == "ProRes422", result=result):
                raise RuntimeError("unexpected generated clip")
            server_sha = publish(run, path, source)
            clip = {"path": str(path), "frames": frames, "seed": seed, "bytes": source["size"],
                    "durationSeconds": frames / 30, "sourceSha256": source["sha256"], "serverSha256": server_sha,
                    "makeSeconds": result["elapsedSeconds"]}
            if number == 1:
                inspect_clip(run, args, clip)
                last_inspected = number
        peaks = disk.record(run, path.name)
        manifest["clips"].append(clip)
        manifest["encodedBytes"] += source["size"]
        bytes_per_frame = source["size"] / frames
        manifest["plannedCumulativeWriteBytes"] = planned
        (case / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        run.emit("media_clip", **clip, cumulative_encoded_bytes=manifest["encodedBytes"], disk_peak=peaks)
        if not run.check("media cache stays inside read allowance", peaks.get("cache", 0) - baseline_cache <= args.read_budget_bytes,
                         cache_delta_bytes=peaks.get("cache", 0) - baseline_cache, allowance_bytes=args.read_budget_bytes):
            raise RuntimeError("media reads exceeded cache budget")
        if manifest["encodedBytes"] > args.target_bytes + args.max_overshoot_bytes:
            raise RuntimeError("encoded project exceeded allowed overshoot")
        if run.failures:
            raise RuntimeError("stop media generation after failed checks")
    if last_inspected != len(manifest["clips"]):
        with DiskSamples(args, case, run, "last clip native scrubs") as disk:
            inspect_clip(run, args, manifest["clips"][-1])
        peaks = disk.record(run, "last clip native scrubs")
        run.check("last clip cache stays inside read allowance", peaks.get("cache", 0) - baseline_cache <= args.read_budget_bytes)
    export = case / "export-2s.mov"
    with DiskSamples(args, case, run, "trimmed HEVC export") as disk:
        result, source = paused_source(run, args, export, "HEVC 2-second export",
                                       ["/usr/bin/avconvert", "--source", manifest["clips"][0]["path"], "--output", export,
                                        "--preset", "PresetHEVC1920x1080", "--start", "0", "--duration", "2", "--replace"],
                                       json_result=False)
        publish(run, export, source)
        inspected = parsed(run, "inspect trimmed HEVC export", [args.media, "inspect", export])
        assert_inspection(run, "trimmed export", inspected, 2, args.width, args.height)
    peaks = disk.record(run, "trimmed HEVC export")
    run.check("export cache stays inside read allowance", peaks.get("cache", 0) - baseline_cache <= args.read_budget_bytes)
    manifest["export"] = {"path": str(export), "bytes": source["size"], "sha256": source["sha256"], "durationSeconds": inspected["durationSeconds"]}
    manifest_path = case / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    manifest_source = common.metadata(manifest_path)
    run.emit("manifest_source", **manifest_source)
    publish(run, manifest_path, manifest_source)
    run.check("real encoded project reaches requested size", args.target_bytes <= manifest["encodedBytes"] <= args.target_bytes + args.max_overshoot_bytes,
              encoded_bytes=manifest["encodedBytes"], target_bytes=args.target_bytes)
    run.emit("media_manifest", **manifest)
    return case


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--root", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--case", default="media-50gb")
    parser.add_argument("--resume-evidence", action="append",
                        help="resume using prior JSONL history/clip evidence; repeat for every earlier continuation")
    parser.add_argument("--target-bytes", type=int, default=50_000_000_000)
    parser.add_argument("--frames-per-clip", type=int, default=1200)
    parser.add_argument("--estimated-bytes-per-frame", type=float)
    parser.add_argument("--width", type=int, default=1920)
    parser.add_argument("--height", type=int, default=1080)
    parser.add_argument("--seed", type=int, default=19835)
    parser.add_argument("--media", default="/private/tmp/voidfs-media")
    parser.add_argument("--drive")
    parser.add_argument("--mount")
    parser.add_argument("--void")
    parser.add_argument("--metadata")
    parser.add_argument("--state-dir", default=os.environ.get("VOIDFS_STATE_DIR"))
    parser.add_argument("--server-dir")
    parser.add_argument("--timeout", type=float, default=180)
    parser.add_argument("--drain-timeout", type=float, default=300)
    parser.add_argument("--max-overshoot-bytes", type=int, default=2_000_000_000)
    parser.add_argument("--read-budget-bytes", type=int, default=8_000_000_000)
    parser.add_argument("--other-budget-bytes", type=int, default=10_000_000_000)
    parser.add_argument("--write-budget-bytes", type=int, default=190_000_000_000)
    args = parser.parse_args()
    args.row = "large-media"
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]{0,62}", args.case):
        parser.error("--case must be a plain scratch folder name")
    if args.target_bytes <= 0 or args.frames_per_clip < 60 or args.timeout > 180 or args.drain_timeout > 300:
        parser.error("positive target, at least 60 frames, timeout <=180 and drain timeout <=300 required")
    if any((args.drive, args.mount, args.void)) and not all((args.drive, args.mount, args.void, args.metadata, args.state_dir, args.server_dir)):
        parser.error("server checks require drive/mount/void/metadata/state-dir/server-dir")
    Path(args.root).mkdir(parents=True, exist_ok=True)
    run = common.Runner(args)
    case = run.root / args.case
    try:
        if args.drive and not run.drive_id:
            raise RuntimeError("drive alias did not resolve")
        case = media_row(run, args)
    except Exception as error:
        run.check("media row completed", False, exception=type(error).__name__, error=str(error))
        import traceback
        run.emit("exception", traceback=traceback.format_exc())
    run.emit("summary", row=args.row, case=str(case), failures=run.failures, limitations=run.limits,
             result="fail" if run.failures else "pass with stated limitations" if run.limits else "pass")
    run.log.close()
    return 1 if run.failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
