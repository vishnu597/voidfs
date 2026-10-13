#!/usr/bin/python3
# SPDX-License-Identifier: Apache-2.0
"""Rerunnable git/SQLite application rows and byte/metadata evidence for a test mount.

APFS control:
  /usr/bin/python3 apps/macos/scripts/app-matrix.py --row git --root /tmp/control --out git-apfs.jsonl
Test mount (credentials and isolated daemon state come from the environment):
  app-matrix.py --row sqlite --root /tmp/matrix --mount /tmp/matrix --drive matrix \
    --void target/item6/debug/void --out sqlite-fskit.jsonl

The runner never starts a server, mounts a volume, registers an app, or uses a user's database.
Each row creates its own scratch subtree; --row collect only reads --root. JSONL contains every
operation, elapsed time, exit code, checks, file hashes and metadata. Failures exit nonzero.
Server checks require --drive, --mount and --void together. --metadata optionally names an SDK
probe taking DRIVE attrs KEY and returning JSON with mode, mtime and base64 xattrs, like Attributes.
"""

import argparse
import base64
import contextlib
import ctypes
import datetime
import hashlib
import json
import os
from pathlib import Path
import select
import signal
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
from types import SimpleNamespace
from unittest.mock import patch


SQLITE = "/usr/bin/sqlite3"
GIT = "/usr/bin/git"


def xattrs(path):
    # Apple's system Python omits os.listxattr/getxattr. Darwin's native calls avoid a
    # subprocess per attribute and still distinguish a missing attribute from an empty one.
    if hasattr(os, "listxattr"):
        return {name: os.getxattr(path, name, follow_symlinks=False)
                for name in os.listxattr(path, follow_symlinks=False)}
    libc = ctypes.CDLL(None, use_errno=True)
    libc.listxattr.argtypes = [ctypes.c_char_p, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int]
    libc.listxattr.restype = ctypes.c_ssize_t
    libc.getxattr.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_void_p, ctypes.c_size_t,
                             ctypes.c_uint32, ctypes.c_int]
    libc.getxattr.restype = ctypes.c_ssize_t
    encoded = os.fsencode(path)
    size = libc.listxattr(encoded, None, 0, 1)
    if size < 0:
        code = ctypes.get_errno()
        raise OSError(code, os.strerror(code), str(path))
    names = ctypes.create_string_buffer(size)
    actual = libc.listxattr(encoded, names, size, 1)
    if actual < 0:
        code = ctypes.get_errno()
        raise OSError(code, os.strerror(code), str(path))
    result = {}
    for name in names.raw[:actual].split(b"\0"):
        if not name:
            continue
        length = libc.getxattr(encoded, name, None, 0, 0, 1)
        if length < 0:
            code = ctypes.get_errno()
            raise OSError(code, os.strerror(code), str(path))
        value = ctypes.create_string_buffer(length)
        actual = libc.getxattr(encoded, name, value, length, 0, 1)
        if actual < 0:
            code = ctypes.get_errno()
            raise OSError(code, os.strerror(code), str(path))
        result[os.fsdecode(name)] = value.raw[:actual]
    return result


def digest(path):
    value = hashlib.sha256()
    with open(path, "rb") as source:
        for block in iter(lambda: source.read(1 << 20), b""):
            value.update(block)
    return value.hexdigest()


def metadata(path, content=True):
    info = os.lstat(path)
    record = {"path": str(path), "size": info.st_size, "mode": oct(stat.S_IMODE(info.st_mode)),
              "mtime_ns": info.st_mtime_ns, "type": "directory" if stat.S_ISDIR(info.st_mode) else
              "symlink" if stat.S_ISLNK(info.st_mode) else "file", "xattrs": {}}
    if stat.S_ISLNK(info.st_mode):
        record["target"] = os.readlink(path)
    elif stat.S_ISREG(info.st_mode) and content:
        record["sha256"] = digest(path)
    for name, value in sorted(xattrs(path).items()):
        record["xattrs"][name] = {"size": len(value), "sha256": hashlib.sha256(value).hexdigest()}
    return record


def expected_server_xattrs(attrs, run, key):
    values = {name: base64.b64decode(value) for name, value in attrs.get("xattrs", {}).items()}
    if sys.platform == "darwin" and values.get("com.apple.FinderInfo") == bytes(32):
        del values["com.apple.FinderInfo"]
        run.emit("semantic_normalization", key=key, name="com.apple.FinderInfo", bytes=32,
                 meaning="macOS presents exactly 32 zero FinderInfo bytes as an absent xattr; raw server attributes and history retained")
    return {name: {"size": len(value), "sha256": hashlib.sha256(value).hexdigest()} for name, value in values.items()}


class Runner:
    def __init__(self, args):
        self.args = args
        self.failures = 0
        self.limits = 0
        self.output = Path(args.out).resolve()
        self.output.parent.mkdir(parents=True, exist_ok=True)
        self.log = self.output.open("a", encoding="utf-8")
        self.root = Path(args.root).resolve()
        self.scope = self.root
        self.mount = Path(args.mount).resolve() if args.mount else None
        self.void = str(Path(args.void).resolve()) if args.void else None
        self.drive_id = None
        self.env = os.environ.copy()
        self.env.update({"GIT_TERMINAL_PROMPT": "0", "GIT_AUTHOR_NAME": "voidfs matrix",
                         "GIT_AUTHOR_EMAIL": "matrix@example.invalid", "GIT_COMMITTER_NAME": "voidfs matrix",
                         "GIT_COMMITTER_EMAIL": "matrix@example.invalid"})
        self.emit("run", row=args.row, root=str(self.root), platform=sys.platform,
                  python=sys.version.split()[0], server_checks=bool(args.drive), utc=self.utc())
        self.command("macOS", ["/usr/bin/sw_vers"])
        self.command("git version", [GIT, "--version"])
        self.command("SQLite version", [SQLITE, "--version"])
        if args.drive:
            rc, out, err = self.command("resolve drive alias", [self.void, "--json", "drive", "show", args.drive])
            try:
                resolved = json.loads(out)
                self.drive_id = resolved["driveId"] if rc == 0 else None
            except (ValueError, KeyError):
                self.drive_id = None
            self.check("drive alias resolved to server ID", bool(self.drive_id), alias=args.drive, drive_id=self.drive_id)

    @staticmethod
    def utc():
        return datetime.datetime.now(datetime.timezone.utc).isoformat()

    def emit(self, kind, **record):
        record = {"kind": kind, "utc": self.utc(), **record}
        self.log.write(json.dumps(record, ensure_ascii=False, sort_keys=True) + "\n")
        self.log.flush()
        if kind in ("check", "limit", "summary", "operation"):
            print(json.dumps(record, ensure_ascii=False, sort_keys=True), flush=True)

    def check(self, name, ok, **detail):
        if not ok:
            self.failures += 1
        self.emit("check", name=name, ok=bool(ok), **detail)
        return ok

    def limit(self, name, **detail):
        self.limits += 1
        self.emit("limit", name=name, **detail)

    def command(self, name, argv, cwd=None, expected=(0,), env=None, timeout=None, quiet=False, pass_fds=()):
        start = time.monotonic()
        try:
            process = subprocess.Popen([str(a) for a in argv], cwd=cwd, env=env or self.env,
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                                       errors="replace", start_new_session=True, pass_fds=pass_fds)
            stdout, stderr = process.communicate(timeout=timeout or self.args.timeout)
            rc = process.returncode
        except subprocess.TimeoutExpired as error:
            os.killpg(process.pid, signal.SIGKILL)
            stdout, stderr = process.communicate()
            rc = 124
            stderr += "\nrunner timeout"
        except OSError as error:
            rc, stdout, stderr = 127, "", str(error)
        elapsed = time.monotonic() - start
        if rc not in expected:
            self.failures += 1
        record = {"name": name, "argv": [str(a) for a in argv], "cwd": str(cwd) if cwd else None,
                  "rc": rc, "expected_rc": list(expected), "seconds": round(elapsed, 6),
                  "stdout": stdout, "stderr": stderr}
        if self.args.drive and "--json" in argv and "uploads" in argv and rc == 0:
            try:
                payload = json.loads(stdout)
                prefix = self.key(self.scope).rstrip("/")
                prefix = "" if prefix == "." else prefix + "/"
                original = payload["items"]
                selected = [item for item in original if item["drive"] in (self.args.drive, self.drive_id) and
                            (item["key"].startswith(prefix) or item.get("toKey", "").startswith(prefix))]
                batches = {item["batch"] for item in selected if "batch" in item}
                payload["items"] = selected
                payload["batches"] = [batch for batch in payload.get("batches", []) if batch["id"] in batches]
                record["stdout"] = json.dumps(payload, sort_keys=True)
                record["stdout_filter"] = {"note": "log contains row-scoped items/batches and unchanged global counts; checks receive original parsed response",
                                           "scope": prefix, "original_items": len(original), "logged_items": len(selected)}
            except (ValueError, KeyError, TypeError):
                pass
        if quiet:
            self.log.write(json.dumps({"kind": "operation", "utc": self.utc(), **record}, sort_keys=True) + "\n")
            self.log.flush()
        else:
            self.emit("operation", **record)
        return rc, stdout, stderr

    def git(self, name, directory, *args, **kwargs):
        return self.command(name, [GIT, "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false",
                                  "-c", "maintenance.auto=false", *args], cwd=directory, **kwargs)

    def sql(self, name, path, sql, **kwargs):
        return self.command(name, [SQLITE, "-batch", str(path), sql], **kwargs)

    def snapshot(self, root):
        paths = [root]
        for directory, folders, files in os.walk(root, followlinks=False):
            folders.sort()
            files.sort()
            paths.extend(Path(directory) / name for name in folders + files)
        records = []
        for path in paths:
            try:
                item = metadata(path)
                item["relative"] = str(path.relative_to(root))
                records.append(item)
                self.emit("file", **item)
            except OSError as error:
                self.check("read resulting bytes and metadata", False, path=str(path), error=str(error))
        self.check("no AppleDouble files", not any(Path(p["path"]).name.startswith("._") for p in records),
                   root=str(root), entries=len(records))
        return records

    def drain(self, name):
        if not self.args.drive:
            return True
        if not self.drive_id:
            self.check(name + ": drive ID known", False)
            return False
        start = time.monotonic()
        deadline = start + self.args.drain_timeout
        # Three samples span the 2s quiet period. Scope entries to this row, because another
        # app can keep the same daemon busy; retain the whole-daemon counters as evidence.
        clean = 0
        last = None
        while time.monotonic() < deadline:
            rc, out, err = self.command(name + ": status", [self.void, "--json", "status"], quiet=True)
            if rc:
                return False
            rc2, info, err2 = self.command(name + ": info", [self.void, "--json", "daemon", "info"], quiet=True)
            if rc2:
                return False
            rc3, queue, err3 = self.command(name + ": uploads", [self.void, "--json", "uploads"], quiet=True)
            if rc3:
                return False
            try:
                status, details = json.loads(out), json.loads(info)
                uploads = status["uploads"]
                journal = details["journal"]
                prefix = self.key(self.scope).rstrip("/")
                prefix = "" if prefix == "." else prefix + "/"
                relevant = [item for item in json.loads(queue)["items"] if item["drive"] == self.drive_id
                            and (item["key"].startswith(prefix) or item.get("toKey", "").startswith(prefix))]
                last = {"uploads": uploads, "journal": journal,
                        "memoryOnlyBytes": details["memoryOnlyBytes"], "restartSafe": details["restartSafe"],
                        "scope": prefix, "scope_items": relevant}
            except (ValueError, KeyError) as error:
                self.check(name + ": parse daemon status", False, error=str(error), stdout=out, info=info)
                return False
            if any(item["state"] == "failed" or item.get("conflict") for item in relevant):
                self.check(name + ": queue has no failed/conflicted saves", False, status=last)
                self.command(name + ": upload evidence", [self.void, "--json", "uploads", "--all"])
                return False
            done = not relevant
            clean = clean + 1 if done else 0
            if clean >= 3:
                self.check(name + ": publication drained", True, seconds=round(time.monotonic() - start, 6), status=last)
                self.command(name + ": upload evidence", [self.void, "--json", "uploads", "--all"])
                return True
            time.sleep(1.1)
        self.check(name + ": publication drained", False, seconds=round(time.monotonic() - start, 6), status=last)
        self.command(name + ": upload evidence", [self.void, "--json", "uploads", "--all"])
        return False

    def key(self, path):
        return str(Path(path).resolve().relative_to(self.mount))

    def server_hash(self, key, version=None):
        argv = [self.void, "show", self.args.drive, key]
        if version:
            argv += ["--version", version]
        start = time.monotonic()
        sha, size = hashlib.sha256(), 0
        with tempfile.TemporaryFile() as errors:
            process = subprocess.Popen(argv, stdout=subprocess.PIPE, stderr=errors, env=self.env)
            deadline = start + self.args.timeout
            timed_out = False
            while True:
                left = deadline - time.monotonic()
                if left <= 0 or not select.select([process.stdout], [], [], max(0, left))[0]:
                    process.kill()
                    timed_out = True
                    break
                block = os.read(process.stdout.fileno(), 1 << 20)
                if not block:
                    break
                sha.update(block)
                size += len(block)
            rc = process.wait()
            process.stdout.close()
            errors.seek(0)
            error = errors.read().decode(errors="replace")
        if timed_out:
            rc = 124
        if rc:
            self.failures += 1
        record = {"argv": argv, "rc": rc, "size": size, "sha256": sha.hexdigest(),
                  "seconds": round(time.monotonic() - start, 6), "stderr": error}
        self.emit("server_bytes", key=key, **record)
        return record

    def history(self, path):
        rc, out, err = self.command("server history", [self.void, "--json", "history", self.args.drive,
                                                       self.key(path), "--all"], quiet=True)
        if rc:
            return []
        try:
            result = json.loads(out)
            return result["versions"]
        except (ValueError, KeyError) as error:
            self.check("server history parses", False, path=str(path), error=str(error), stdout=out)
            return []

    def reconcile(self, root, records=None):
        if not self.args.drive:
            return
        if not self.drain("row complete"):
            return
        records = records if records is not None else self.snapshot(root)
        # Reading a mapped file can itself make FSKit push pages that had no open descriptor.
        # Read the final local view, drain again, and recheck it after reading the server.
        stable = []
        for item in records:
            if item["type"] == "file":
                final = metadata(Path(item["path"]))
                final["relative"] = item["relative"]
                self.emit("stable_local_file", **final)
                stable.append(final)
            else:
                stable.append(item)
        records = stable
        if not self.drain("after final local reads"):
            return
        count = 0
        for item in records:
            if item["type"] != "file":
                continue
            path = Path(item["path"])
            # .DS_Store is the accepted local-only policy, so no remote object is expected.
            if path.name == ".DS_Store":
                self.limit(".DS_Store stays local", path=str(path))
                continue
            key = self.key(path)
            remote = self.server_hash(key)
            latest_local = digest(path)
            self.check("local bytes stable across server read", latest_local == item["sha256"], key=key,
                       before_sha256=item["sha256"], after_sha256=latest_local)
            self.check("mount and server bytes agree", remote["rc"] == 0 and remote["sha256"] == item["sha256"]
                       and remote["size"] == item["size"], key=key, local_sha256=item["sha256"], server=remote)
            versions = self.history(path)
            self.emit("versions", key=key, versions=versions)
            self.check("published file has a current version", bool(versions) and any(v.get("isLatest") for v in versions), key=key)
            if self.args.metadata:
                rc, out, err = self.command("server metadata", [self.args.metadata, self.args.drive, "attrs", key], quiet=True)
                if rc == 0:
                    try:
                        attrs = json.loads(out)
                        self.emit("server_metadata", key=key, attributes=attrs)
                        if attrs.get("mode") is not None:
                            self.check("mount and server mode agree", int(attrs["mode"], 8) == int(item["mode"], 8), key=key)
                        else:
                            self.limit("server metadata omits mode; mode comparison untested", key=key)
                        if attrs.get("mtime") is not None:
                            stamp = datetime.datetime.fromisoformat(attrs["mtime"].replace("Z", "+00:00")).timestamp()
                            self.check("mount and server mtime agree", abs(stamp - item["mtime_ns"] / 1e9) < 0.001, key=key)
                        else:
                            self.limit("server metadata omits mtime; mtime comparison untested", key=key)
                        xattrs = expected_server_xattrs(attrs, self, key)
                        self.check("mount and server xattrs agree", xattrs == item["xattrs"], key=key,
                                   local=item["xattrs"], server=xattrs)
                    except (ValueError, TypeError, KeyError) as error:
                        self.check("server metadata parses", False, key=key, error=str(error), stdout=out)
            count += 1
        self.emit("reconciliation", root=str(root), files=count, server_metadata=bool(self.args.metadata))
        if not self.args.metadata:
            self.limit("server mode/xattrs not exposed by CLI show; SDK metadata probe required")

    def recovery(self, root):
        path = root / "recovery.txt"
        before, after = b"matrix earlier version\n", b"matrix later version\n"
        path.write_bytes(before)
        if not self.args.drive:
            self.limit("server restore untested on APFS control")
            return
        if not self.drain("recovery first save"):
            return
        versions = self.history(path)
        current = next((v for v in versions if v.get("isLatest")), None)
        if not self.check("first save has a recoverable version", current is not None, versions=versions):
            return
        path.write_bytes(after)
        if not self.drain("recovery second save"):
            return
        self.command("restore earlier save", [self.void, "--json", "restore", self.args.drive,
                                              self.key(path), "--version", current["versionId"]])
        start = time.monotonic()
        ok = False
        while time.monotonic() - start < 5:
            try:
                ok = path.read_bytes() == before
            except OSError:
                pass
            if ok:
                break
            time.sleep(0.05)
        self.check("restored earlier version opens with exact bytes", ok, seconds=round(time.monotonic() - start, 6),
                   source_sha256=hashlib.sha256(before).hexdigest(), metadata=metadata(path))
        self.history(path)

    def sqlite_recovery(self, path):
        if not self.args.drive:
            self.limit("SQLite server version restore untested on APFS control")
            return
        if not self.drain("SQLite recovery initial version"):
            return
        before = digest(path)
        versions = self.history(path)
        current = next((v for v in versions if v.get("isLatest")), None)
        if not self.check("SQLite database has a recoverable version", current is not None, versions=versions):
            return
        self.sql("SQLite save a later database version", path,
                 "INSERT INTO t(value) VALUES('later'); SELECT count(*) FROM t;")
        if not self.drain("SQLite recovery later save"):
            return
        self.command("restore earlier SQLite database", [self.void, "--json", "restore", self.args.drive,
                                                          self.key(path), "--version", current["versionId"]])
        start = time.monotonic()
        same = False
        while time.monotonic() - start < 5:
            try:
                same = digest(path) == before
            except OSError:
                pass
            if same:
                break
            time.sleep(0.05)
        self.check("SQLite restored database SHA-256", same, expected=before, actual=digest(path),
                   seconds=round(time.monotonic() - start, 6))
        rc, out, err = self.sql("SQLite opens restored earlier database", path,
                               "PRAGMA integrity_check; SELECT group_concat(value) FROM t;")
        self.check("SQLite restored version opens with earlier rows", rc == 0 and out.splitlines() == ["ok", "first,second"], stdout=out)


def git_row(run):
    root = run.root / ("git-" + str(time.time_ns()))
    root.mkdir(parents=True)
    run.scope = root
    repo = root / "repo"
    source = Path(run.args.source).resolve()
    rc, out, err = run.git("source commit", source, "rev-parse", "HEAD")
    source_commit = out.strip()
    source_hashes = {}
    # Read the committed source, so changes in the user's working tree cannot affect the
    # expected checkout or expose ignored files. Archive streams without copying the repo.
    with tempfile.TemporaryFile() as errors:
        archive = subprocess.Popen([GIT, "-C", str(source), "archive", source_commit], stdout=subprocess.PIPE, stderr=errors)
        with tarfile.open(fileobj=archive.stdout, mode="r|") as entries:
            for item in entries:
                if item.isfile():
                    content = entries.extractfile(item)
                    sha = hashlib.sha256()
                    for block in iter(lambda: content.read(1 << 20), b""):
                        sha.update(block)
                    source_hashes[item.name] = sha.hexdigest()
                    run.emit("source_file", source_commit=source_commit, relative=item.name, size=item.size, sha256=sha.hexdigest())
        archive.stdout.close()
        run.check("source archive read", archive.wait() == 0, source_commit=source_commit, files=len(source_hashes))
    rc, out, err = run.git("clone local code repo without hard links", root, "clone", "--no-local", source, repo)
    if rc:
        run.emit("untested", name="git operations after clone", reason="clone prerequisite failed",
                 operations=["status", "edit", "add", "commit", "rebase", "stash", "gc", "repack", "checkout",
                             "worktree add", "index.lock", "symlink clone", "fsck", "restore"])
        run.snapshot(root)
        return root
    mismatches = []
    for relative, expected in source_hashes.items():
        try:
            if digest(repo / relative) != expected:
                mismatches.append(relative)
        except OSError as error:
            mismatches.append({"path": relative, "error": str(error)})
    run.check("cloned source SHA-256 manifest matches", not mismatches, files=len(source_hashes), mismatches=mismatches)
    run.git("initial status", repo, "status", "--porcelain=v1")
    run.git("initial fsck", repo, "fsck", "--full")
    run.git("fixture config name", repo, "config", "user.name", "voidfs matrix")
    run.git("fixture config email", repo, "config", "user.email", "matrix@example.invalid")
    run.git("fixture branch", repo, "switch", "-c", "matrix-base")
    changed = repo / "matrix-file.txt"
    changed.write_text("first matrix edit\n", encoding="utf-8")
    run.git("add new code file", repo, "add", "matrix-file.txt")
    run.git("commit edit", repo, "commit", "-m", "tests: matrix first edit")
    run.git("feature branch", repo, "switch", "-c", "matrix-feature")
    (repo / "matrix-feature.txt").write_text("feature\n", encoding="utf-8")
    run.git("stage feature", repo, "add", "matrix-feature.txt")
    run.git("commit feature", repo, "commit", "-m", "tests: matrix feature")
    run.git("checkout base branch", repo, "checkout", "matrix-base")
    (repo / "matrix-base.txt").write_text("base\n", encoding="utf-8")
    run.git("stage base", repo, "add", "matrix-base.txt")
    run.git("commit base", repo, "commit", "-m", "tests: matrix base")
    run.git("checkout feature branch", repo, "checkout", "matrix-feature")
    run.git("rebase feature onto base", repo, "rebase", "matrix-base")
    run.check("rebase preserves both edits", (repo / "matrix-base.txt").read_bytes() == b"base\n"
              and (repo / "matrix-feature.txt").read_bytes() == b"feature\n")
    changed.write_text("stashed edit\n", encoding="utf-8")
    run.git("stash dirty file", repo, "stash", "push", "-m", "matrix stash")
    run.check("stash restores committed bytes", changed.read_bytes() == b"first matrix edit\n")
    run.git("stash pop", repo, "stash", "pop")
    run.check("stash pop restores edited bytes", changed.read_bytes() == b"stashed edit\n")
    run.git("stage stashed edit", repo, "add", "matrix-file.txt")
    run.git("commit stashed edit", repo, "commit", "-m", "tests: matrix stashed edit")
    worktree = root / "worktree"
    run.git("worktree add", repo, "worktree", "add", "--detach", worktree, "HEAD")
    if worktree.exists():
        run.git("worktree status", worktree, "status", "--porcelain=v1")
        run.check("worktree bytes", (worktree / "matrix-file.txt").read_bytes() == b"stashed edit\n")
        run.git("worktree remove", repo, "worktree", "remove", worktree)
    lock = repo / ".git" / "index.lock"
    lock.write_bytes(b"matrix lock owner\n")
    rc, out, err = run.git("index.lock contention", repo, "add", "matrix-file.txt", expected=(128,))
    run.check("index.lock refusal names existing lock", rc == 128 and "index.lock" in err
              and lock.read_bytes() == b"matrix lock owner\n", stderr=err)
    lock.unlink()
    run.git("index lock release", repo, "add", "matrix-file.txt")
    run.git("gc", repo, "gc")
    run.git("repack", repo, "repack", "-ad")
    run.git("final fsck", repo, "fsck", "--full")
    rc, out, err = run.git("final status", repo, "status", "--porcelain=v1")
    run.check("final worktree clean", rc == 0 and not out.strip(), stdout=out)

    # The source fixture is APFS; a clone onto the mount probes Git's actual symlink fallback.
    with tempfile.TemporaryDirectory(prefix="voidfs-git-symlink-") as temporary:
        fixture = Path(temporary)
        run.git("symlink source init", fixture, "init", "--initial-branch=main")
        (fixture / "target.txt").write_text("symlink target\n", encoding="utf-8")
        os.symlink("target.txt", fixture / "link.txt")
        run.git("symlink source add", fixture, "add", "target.txt", "link.txt")
        run.git("symlink source commit", fixture, "commit", "-m", "tests: symlink fixture")
        dest = root / "symlinks"
        run.git("clone symlink fixture", root, "clone", "--no-local", fixture, dest)
        if dest.exists():
            rc, out, err = run.git("core.symlinks selected by clone", dest, "config", "--get", "core.symlinks", expected=(0, 1))
            link = dest / "link.txt"
            if os.path.islink(link):
                run.check("symlink checkout preserves link", os.readlink(link) == "target.txt")
            elif link.is_file():
                run.check("unsupported symlink checkout preserves target text", out.strip() == "false"
                          and link.read_bytes() == b"target.txt", core_symlinks=out.strip())
                run.limit("Git core.symlinks=false checks symbolic links out as ordinary files")
            else:
                run.check("symlink checkout exists", False)
            run.git("symlink clone fsck", dest, "fsck", "--full")
            run.git("symlink clone status", dest, "status", "--porcelain=v1")
    run.recovery(root)
    records = run.snapshot(root)
    run.reconcile(root, records)
    return root


@contextlib.contextmanager
def live_sqlite(run, name, path, sql, expected_after=None):
    start = time.monotonic()
    process = subprocess.Popen([SQLITE, "-batch", str(path)], stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=run.env)
    try:
        process.stdin.write((sql + "\n.print matrix-ready\n").encode())
        process.stdin.flush()
        initial = b""
        ready = False
        deadline = time.monotonic() + run.args.timeout
        while time.monotonic() < deadline:
            if not select.select([process.stdout], [], [], 0.1)[0]:
                if process.poll() is not None:
                    break
                continue
            block = os.read(process.stdout.fileno(), 65536)
            if not block:
                break
            initial += block
            if b"matrix-ready\n" in initial:
                ready = True
                break
        run.check(name + ": transaction ready", ready, pid=process.pid, stdout=initial.decode(errors="replace"))
        yield process, ready
        if process.poll() is None:
            process.stdin.write(b".quit\n")
            process.stdin.flush()
        stdout, stderr = process.communicate(timeout=run.args.timeout)
        run.emit("operation", name=name, argv=[SQLITE, "-batch", str(path)], sql=sql,
                 rc=process.returncode, seconds=round(time.monotonic() - start, 6),
                 stdout=(initial + stdout).decode(errors="replace"), stderr=stderr.decode(errors="replace"))
        if process.returncode not in (0, -9):
            run.failures += 1
        if expected_after is not None:
            run.check(name + ": open connection keeps its original rows", stdout.decode().strip() == expected_after,
                      initial=initial.decode(errors="replace"), after=stdout.decode(errors="replace"))
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()


def sqlite_row(run):
    root = run.root / ("sqlite-" + str(time.time_ns()))
    root.mkdir(parents=True)
    run.scope = root
    rollback = root / "rollback.db"
    run.sql("rollback journal transaction", rollback,
            "PRAGMA journal_mode=DELETE; CREATE TABLE t(id INTEGER PRIMARY KEY, value TEXT);"
            "BEGIN IMMEDIATE; INSERT INTO t(value) VALUES('first'),('second'); COMMIT; SELECT count(*) FROM t;")
    rc, out, err = run.sql("rollback journal integrity", rollback, "PRAGMA integrity_check; SELECT group_concat(value) FROM t;")
    run.check("rollback journal resulting rows", rc == 0 and out.splitlines() == ["ok", "first,second"], stdout=out)
    run.sqlite_recovery(rollback)
    for mode in ("DELETE", "WAL"):
        path = root / (mode.lower() + ".db")
        rc, out, err = run.sql(mode + " setup", path,
                              "PRAGMA journal_mode=" + mode + "; CREATE TABLE t(value TEXT); INSERT INTO t VALUES('initial');")
        run.check(mode + " selected journal mode", rc == 0 and out.strip().lower() == mode.lower(), stdout=out)
        with live_sqlite(run, mode + " long-running writer", path,
                         "BEGIN IMMEDIATE; INSERT INTO t VALUES('held');") as (writer, ready):
            if ready:
                rc, out, err = run.sql(mode + " competing writer blocked", path,
                                      "INSERT INTO t VALUES('blocked');", expected=(1, 5))
                run.check(mode + " contention reports database locked", rc in (1, 5) and "locked" in err, stderr=err)
                rc, out, err = run.sql(mode + " reader during write transaction", path, "SELECT group_concat(value) FROM t;")
                run.check(mode + " reader sees only committed rows", rc == 0 and out.strip() == "initial", stdout=out)
                # The hold spans the mount's 2s quiet publication period.
                time.sleep(2.2)
                writer.stdin.write(b"COMMIT;\n")
                writer.stdin.flush()
        rc, out, err = run.sql(mode + " competing writer after release", path,
                              "PRAGMA busy_timeout=3000; INSERT INTO t VALUES('after'); SELECT group_concat(value) FROM t;")
        run.check(mode + " long transaction committed intact", rc == 0 and out.splitlines() == ["3000", "initial,held,after"], stdout=out)
        with live_sqlite(run, mode + " killed uncommitted writer", path,
                         "BEGIN IMMEDIATE; INSERT INTO t VALUES('must-rollback');") as (writer, ready):
            if ready:
                writer.kill()
        rc, out, err = run.sql(mode + " reopen after killed writer", path,
                              "PRAGMA integrity_check; SELECT group_concat(value) FROM t;")
        run.check(mode + " crash rolls back uncommitted row", rc == 0 and out.splitlines() == ["ok", "initial,held,after"], stdout=out)
        if mode == "WAL":
            rc, out, err = run.sql("WAL checkpoint truncate", path, "PRAGMA wal_checkpoint(TRUNCATE); PRAGMA integrity_check;")
            run.check("WAL checkpoint and integrity", rc == 0 and out.splitlines() == ["0|0|0", "ok"], stdout=out)

    # Read-only peer replacement: a DB is a whole file snapshot, not a shared network DB.
    if run.args.drive and run.drain("before independent SQLite client"):
        path = root / "peer.db"
        run.sql("peer database initial", path, "CREATE TABLE t(value TEXT); INSERT INTO t VALUES('old');")
        if run.drain("initial peer database"):
            with tempfile.TemporaryDirectory(prefix="voidfs-sqlite-peer-") as temporary:
                peer_dir = Path(temporary)
                replacement = peer_dir / "peer.db"
                run.sql("independent peer database", replacement, "CREATE TABLE t(value TEXT); INSERT INTO t VALUES('new');")
                peer_env = run.env.copy()
                peer_env["VOIDFS_STATE_DIR"] = str(peer_dir / "state")
                commit_monotonic = time.monotonic()
                with live_sqlite(run, "SQLite read transaction across peer replacement", path,
                                 "BEGIN; SELECT value FROM t;", expected_after="old") as (reader, ready):
                    if ready:
                        rc, out, err = run.command("independent client replaces database", [run.void, "--json", "upload",
                                                   replacement, run.args.drive + ":/" + run.key(root)], env=peer_env)
                        committed = time.time()
                        commit_monotonic = time.monotonic()
                        reader.stdin.write(b"SELECT value FROM t; COMMIT;\n")
                        reader.stdin.flush()
                        run.limit("SQLite connection opened before peer replacement keeps its file snapshot; concurrent cross-Mac writers unsupported",
                                  commit_returned_unix=committed)
                start = commit_monotonic
                latest = False
                last = ""
                while time.monotonic() - start < 5:
                    rc, last, err = run.sql("SQLite new connection after peer replacement", path,
                                            "PRAGMA integrity_check; SELECT value FROM t;", quiet=True)
                    if rc == 0 and last.splitlines() == ["ok", "new"]:
                        latest = True
                        break
                    time.sleep(0.1)
                run.check("SQLite new connection sees peer commit within 5 seconds from commit return", latest,
                          seconds=round(time.monotonic() - start, 6), stdout=last)
    elif not run.args.drive:
        run.limit("independent server client replacement untested on APFS control")
    run.recovery(root)
    records = run.snapshot(root)
    run.reconcile(root, records)
    return root


def drain_self_test(run):
    """Exercise the actual drain path with the alias/ID distinction seen on a live daemon."""
    fake = Runner.__new__(Runner)
    fake.args = SimpleNamespace(drive="matrix-alias", drain_timeout=0.03, timeout=1)
    fake.drive_id = "d-actual-server-id"
    fake.mount = Path("/private/tmp/drain-fixture")
    fake.scope = fake.mount / "row"
    fake.void = "mock-void"
    observed = []
    fake.emit = lambda kind, **data: observed.append({"kind": kind, **data})
    fake.check = lambda name, ok, **data: observed.append({"kind": "check", "name": name, "ok": bool(ok), **data}) or bool(ok)
    items = [{"drive": fake.drive_id, "key": "row/file.txt", "state": "queued"}]

    def command(name, argv, **kwargs):
        if argv[-1] == "status":
            return 0, json.dumps({"uploads": {"queued": 1, "unpublished": 1}}), ""
        if argv[-1] == "info":
            return 0, json.dumps({"journal": {"unpublished": 1}, "memoryOnlyBytes": 0, "restartSafe": True}), ""
        return 0, json.dumps({"items": items}), ""

    fake.command = command
    with patch("time.sleep", return_value=None):
        result = fake.drain("matching row queued under server ID")
    run.check("queued row identified by drive ID prevents a false drain pass", result is False,
              expected_alias=fake.args.drive, expected_drive_id=fake.drive_id, observations=observed[-1:])
    items[:] = [{"drive": fake.drive_id, "key": "row/file.txt", "state": "failed", "error": "fixture failure"}]
    with patch("time.sleep", return_value=None):
        result = fake.drain("matching row failed under server ID")
    run.check("failed row identified by drive ID prevents a false drain pass", result is False, observations=observed[-1:])
    return run.root


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--row", choices=("git", "sqlite", "collect", "self-test"), required=True)
    parser.add_argument("--root", required=True, help="scratch parent; collect reads this exact subtree")
    parser.add_argument("--out", required=True, help="append-only JSONL evidence file")
    parser.add_argument("--source", default=str(Path(__file__).resolve().parents[3]), help="local code repo for git clone")
    parser.add_argument("--drive")
    parser.add_argument("--mount")
    parser.add_argument("--void")
    parser.add_argument("--peer", help="reserved for mount_peer coherence commands")
    parser.add_argument("--metadata", help="SDK attributes probe binary: DRIVE attrs KEY → JSON")
    parser.add_argument("--timeout", type=float, default=180)
    parser.add_argument("--drain-timeout", type=float, default=300)
    args = parser.parse_args()
    if any((args.drive, args.mount, args.void)) and not all((args.drive, args.mount, args.void)):
        parser.error("--drive, --mount and --void are required together")
    if args.row != "collect":
        Path(args.root).mkdir(parents=True, exist_ok=True)
    run = Runner(args)
    try:
        if args.drive and not run.drive_id:
            root = run.root
        elif args.row == "self-test":
            root = drain_self_test(run)
        elif args.row == "git":
            root = git_row(run)
        elif args.row == "sqlite":
            root = sqlite_row(run)
        else:
            root = run.root
            run.reconcile(root, run.snapshot(root))
    except Exception as error:
        run.check("runner completed", False, exception=type(error).__name__, error=str(error))
        import traceback
        run.emit("exception", traceback=traceback.format_exc())
        root = run.root
    run.emit("summary", row=args.row, root=str(root), failures=run.failures, limitations=run.limits,
             result="fail" if run.failures else "pass with stated limitations" if run.limits else "pass")
    run.log.close()
    return 1 if run.failures else 0


if __name__ == "__main__":
    sys.exit(main())
