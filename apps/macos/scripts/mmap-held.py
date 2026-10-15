#!/usr/bin/python3
# SPDX-License-Identifier: Apache-2.0
"""Compare held mmap stores with the daemon and server without another mount open.

Compile bench/results/mac-fskit-writable/mmapheld.c and pass its binary as --probe.
This runner owns a fresh child of an existing test mount. It never mounts a volume,
starts services, registers an app, or calls sync. Only the after-msync case asks the
probe for one explicit msync, before a later generation is stored without a flush.

Every phase samples at 0.1, 1, 3 and 5 seconds. Early publication lag is recorded;
the final sample must have the expected bytes in every available local view, the
daemon and the server. A final observation ending more than 0.25 seconds after
its 5-second target is inconclusive. Exit 1 means a byte mismatch, 2 a setup,
protocol or timing failure.
"""

import argparse
import datetime
import hashlib
import http.client
import json
import os
from pathlib import Path
import select
import signal
import socket
import subprocess
import sys
import tempfile
import time
import uuid


FILE_SIZE = 32768
MARKER_SIZE = 128
SAMPLE_TIMES = (0.1, 1.0, 3.0, 5.0)
FINAL_SAMPLE_TOLERANCE = 0.25
TIMEOUT = 45
CASES = ("held", "close-first", "unmap-first", "after-msync")


class HarnessError(RuntimeError):
    pass


def marker(generation, region):
    value = bytearray(((generation >> ((index % 8) * 8))
                       ^ (0xa5 + index * 37 + region * 83)) & 0xff
                      for index in range(MARKER_SIZE))
    header = ("VOIDFS_MMAPHELD generation=%020d region=%d "
              % (generation, region)).encode("ascii")
    value[:len(header)] = header
    return bytes(value)


def expected_bytes(generation, last_page_offset):
    value = bytearray(FILE_SIZE)
    value[:MARKER_SIZE] = marker(generation, 0)
    value[last_page_offset:last_page_offset + MARKER_SIZE] = marker(generation, 1)
    return bytes(value)


def byte_view(value, expected, last_page_offset):
    return {"size": len(value), "sha256": hashlib.sha256(value).hexdigest(),
            "matches_generation": value == expected,
            "first_hex": value[:MARKER_SIZE].hex(),
            "last_page_hex": value[last_page_offset:last_page_offset + MARKER_SIZE].hex()}


class UnixHTTP(http.client.HTTPConnection):
    def __init__(self, path):
        super().__init__("localhost", timeout=TIMEOUT)
        self.socket_path = str(path)

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(self.timeout)
        self.sock.connect(self.socket_path)


class Daemon:
    """All RPCs stay in this process, which owns the read-only session."""

    def __init__(self, run):
        self.run = run
        self.http = UnixHTTP(run.args.state_dir / "daemon.sock")
        self.session = None
        response = self.request("POST", "/v1/fs/sessions",
                                {"version": 1, "drive": run.args.drive, "readOnly": True})
        if (not isinstance(response, dict)
                or response.get("version") != 1 or not response.get("readOnly")
                or not response.get("id") or not response.get("generation")):
            raise HarnessError("daemon returned an invalid read-only session")
        self.session = response

    def request(self, method, path, payload=None, binary=False):
        started = time.monotonic_ns()
        headers = {"content-type": "application/json"}
        if self.session:
            headers["x-voidfs-generation"] = str(self.session["generation"])
        body = json.dumps(payload).encode() if payload is not None else None
        self.http.request(method, path, body=body, headers=headers)
        response = self.http.getresponse()
        value = response.read(FILE_SIZE + 65537)
        self.run.emit("rpc", method=method, operation=path.rsplit("/", 1)[-1],
                      status=response.status, started_monotonic_ns=started,
                      ended_monotonic_ns=time.monotonic_ns(), bytes=len(value))
        if response.status != 200:
            raise HarnessError("daemon RPC %s answered %d: %s"
                               % (path, response.status, value.decode(errors="replace")))
        return value if binary else json.loads(value)

    def call(self, operation, payload=None, binary=False, method="POST"):
        return self.request(method, "/v1/fs/" + self.session["id"] + "/" + operation,
                            payload, binary)

    def read(self, relative):
        ino = self.session["root"]
        for component in relative.parts:
            ino = self.call("lookup", {"parent": ino, "name": component})["ino"]
        opened = self.call("open", {"ino": ino, "write": False})
        try:
            value = self.call("read?fh=%d&offset=0&length=%d" % (opened["fh"], FILE_SIZE),
                              binary=True, method="GET")
            return value, opened["attr"]
        finally:
            self.call("close", {"fh": opened["fh"]})

    def close(self):
        try:
            if self.session:
                self.call("release", {})
                self.session = None
        finally:
            self.http.close()


class Probe:
    def __init__(self, run, path, case):
        self.run, self.case = run, case
        self.buffer = bytearray()
        self.fd_open, self.mapped = True, True
        self.generation = 1
        self.stderr = tempfile.TemporaryFile()
        self.process = subprocess.Popen([str(run.args.probe), str(path)],
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=self.stderr, start_new_session=True)
        self.run.emit("probe_start", case=case, path=str(path), pid=self.process.pid)
        try:
            self.ready = self.until("ready")
            self.last_page_offset = self.ready["last_page_offset"]
            if (self.ready.get("file_size") != FILE_SIZE
                    or self.ready.get("marker_size") != MARKER_SIZE
                    or not MARKER_SIZE <= self.last_page_offset <= FILE_SIZE - MARKER_SIZE):
                raise HarnessError("probe returned invalid file or marker geometry")
        except (OSError, ValueError, KeyError, TypeError, HarnessError):
            if self.process.poll() is None:
                os.killpg(self.process.pid, signal.SIGKILL)
            self.process.wait(timeout=TIMEOUT)
            self.finish()
            raise

    def line(self, deadline):
        while b"\n" not in self.buffer:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise HarnessError("probe response exceeded 45 seconds")
            ready, _, _ = select.select([self.process.stdout], [], [], remaining)
            if not ready:
                raise HarnessError("probe response exceeded 45 seconds")
            chunk = os.read(self.process.stdout.fileno(), 65536)
            if not chunk:
                raise HarnessError("probe exited before its response")
            self.buffer.extend(chunk)
            if len(self.buffer) > 131072:
                raise HarnessError("probe response exceeded its size bound")
        line, _, rest = self.buffer.partition(b"\n")
        self.buffer = bytearray(rest)
        try:
            record = json.loads(line)
            if not isinstance(record, dict):
                raise ValueError("probe record must be an object")
            return record
        except ValueError as error:
            raise HarnessError("invalid probe JSON: " + line.decode(errors="replace")) from error

    def until(self, phase):
        deadline = time.monotonic() + TIMEOUT
        while True:
            record = self.line(deadline)
            self.run.emit("probe", case=self.case, record=record,
                          observed_monotonic_ns=time.monotonic_ns())
            # The read command deliberately reports EBADF after a requested close.
            allowed_bad_fd = (record.get("phase") == "pread" and not self.fd_open
                              and record.get("errno") == 9)
            if record.get("ok") is False and not allowed_bad_fd:
                raise HarnessError("probe syscall failed: " + json.dumps(record))
            if record.get("phase") == phase:
                return record

    def command(self, command):
        started = time.monotonic_ns()
        self.process.stdin.write((command + "\n").encode("ascii"))
        self.process.stdin.flush()
        phase = {"unmap": "munmap"}.get(command, command.split()[0])
        record = self.until(phase)
        if command == "close":
            self.fd_open = False
        elif command == "unmap":
            self.mapped = False
        elif command.startswith("store "):
            self.generation = int(command.split()[1])
        self.run.emit("probe_command", case=self.case, command=command,
                      started_monotonic_ns=started, ended_monotonic_ns=time.monotonic_ns())
        return record

    def close(self):
        try:
            if self.process.poll() is None:
                self.command("quit")
            self.process.wait(timeout=TIMEOUT)
        except (OSError, subprocess.SubprocessError, HarnessError):
            if self.process.poll() is None:
                os.killpg(self.process.pid, signal.SIGKILL)
                self.process.wait(timeout=TIMEOUT)
            raise
        finally:
            self.finish()
        if self.process.returncode:
            raise HarnessError("probe exited nonzero")

    def finish(self):
        # quit/EOF cleanup emits munmap and close records after its acknowledgement.
        errors = []
        try:
            trailing = bytes(self.buffer) + self.process.stdout.read()
            for line in trailing.splitlines():
                try:
                    record = json.loads(line)
                    if not isinstance(record, dict):
                        raise ValueError("probe record must be an object")
                except ValueError:
                    self.run.emit("probe_unparsed", case=self.case,
                                  output=line.decode(errors="replace"), cleanup=True)
                    errors.append("malformed JSON in probe cleanup")
                    continue
                self.run.emit("probe", case=self.case, record=record,
                              observed_monotonic_ns=time.monotonic_ns(), cleanup=True)
                if record.get("ok") is False:
                    errors.append("probe cleanup syscall failed: " + json.dumps(record))
            self.stderr.seek(0)
            stderr = self.stderr.read().decode(errors="replace")
            self.run.emit("probe_exit", case=self.case, rc=self.process.returncode, stderr=stderr)
        finally:
            self.stderr.close()
            self.process.stdin.close()
            self.process.stdout.close()
        if errors:
            raise HarnessError("; ".join(errors))


class Runner:
    def __init__(self, args, output):
        self.args, self.output = args, output
        self.started = time.monotonic_ns()
        self.failures = 0
        self.errors = 0
        self.root = args.mount / ("mmap-held-" + uuid.uuid4().hex)
        self.env = dict(os.environ, VOIDFS_STATE_DIR=str(args.state_dir))

    def emit(self, kind, **record):
        value = {"kind": kind,
                 "utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                 "monotonic_ns": time.monotonic_ns(), **record}
        self.output.write(json.dumps(value, ensure_ascii=False, sort_keys=True) + "\n")
        self.output.flush()
        if kind in ("phase", "check", "error", "summary"):
            print(json.dumps(value, ensure_ascii=False, sort_keys=True), flush=True)

    def server(self, key):
        started = time.monotonic_ns()
        argv = [str(self.args.void), "show", self.args.drive, key]
        process = subprocess.Popen(argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   env=self.env, start_new_session=True)
        try:
            stdout, stderr = process.communicate(timeout=TIMEOUT)
        except subprocess.TimeoutExpired as error:
            os.killpg(process.pid, signal.SIGKILL)
            process.communicate()
            raise HarnessError("server read exceeded 45 seconds") from error
        self.emit("server_read", argv=argv, rc=process.returncode,
                  stderr=stderr.decode(errors="replace"), bytes=len(stdout),
                  started_monotonic_ns=started, ended_monotonic_ns=time.monotonic_ns())
        # A file not published yet is a data observation, including at the deadline.
        if process.returncode and not any(value in stderr.lower()
                                          for value in (b"notfound", b"not found", b"nosuchkey", b"404")):
            raise HarnessError("server read failed: " + stderr.decode(errors="replace"))
        return stdout if process.returncode == 0 else None

    def phase(self, daemon, probe, path, name):
        phase_start = time.monotonic_ns()
        expected = expected_bytes(probe.generation, probe.last_page_offset)
        expected_view = byte_view(expected, expected, probe.last_page_offset)
        key = path.relative_to(self.args.mount).as_posix()
        self.emit("phase", case=probe.case, name=name, generation=probe.generation,
                  fd_open=probe.fd_open, mapped=probe.mapped, key=key,
                  started_monotonic_ns=phase_start, expected=expected_view)
        for target in SAMPLE_TIMES:
            remaining = target - (time.monotonic_ns() - phase_start) / 1e9
            if remaining > 0:
                time.sleep(remaining)
            sampled = time.monotonic_ns()
            local = probe.command("read")
            body, attr = daemon.read(path.relative_to(self.args.mount))
            daemon_view = byte_view(body, expected, probe.last_page_offset)
            remote = self.server(key)
            server_view = (byte_view(remote, expected, probe.last_page_offset)
                           if remote is not None else {"missing": True, "matches_generation": False})
            local_views = []
            if probe.mapped:
                local_views.extend((local["mapping_first"], local["mapping_last_page"]))
            if probe.fd_open:
                local_views.extend((local["samefd_first"], local["samefd_last_page"]))
            local_matches = all(value["matches_generation"] for value in local_views)
            sample_ended = time.monotonic_ns()
            observed_seconds = (sample_ended - phase_start) / 1e9
            self.emit("sample", case=probe.case, phase=name, generation=probe.generation,
                      target_seconds=target, phase_started_monotonic_ns=phase_start,
                      sample_started_monotonic_ns=sampled,
                      actual_seconds=(sampled - phase_start) / 1e9,
                      sample_ended_monotonic_ns=sample_ended,
                      observed_seconds=observed_seconds, expected=expected_view,
                      mount={"new_open": False, "available": bool(local_views),
                             "matches_generation": local_matches if local_views else None,
                             "probe_read": local}, daemon=daemon_view, daemon_attr=attr,
                      server=server_view, daemon_matches_server=remote == body)
            if target == SAMPLE_TIMES[-1]:
                timely = observed_seconds <= target + FINAL_SAMPLE_TOLERANCE
                ok = local_matches and daemon_view["matches_generation"] and server_view["matches_generation"]
                self.emit("check", case=probe.case, phase=name, name="generation matches at 5s target sample",
                          ok=ok if timely else None,
                          result=("PASS" if ok else "FAIL") if timely else "INCONCLUSIVE",
                          generation=probe.generation, target_seconds=target,
                          observed_seconds=observed_seconds,
                          tolerance_seconds=FINAL_SAMPLE_TOLERANCE, timely=timely,
                          mount_matches=local_matches if local_views else None,
                          daemon_matches=daemon_view["matches_generation"],
                          server_matches=server_view["matches_generation"])
                if not timely:
                    raise HarnessError("final sample completed at %.6fs, after the 5s target plus %.2fs tolerance"
                                       % (observed_seconds, FINAL_SAMPLE_TOLERANCE))
                self.failures += int(not ok)

    def case(self, daemon, name):
        path = self.root / (name + ".bin")
        probe = None
        try:
            probe = Probe(self, path, name)
            if name == "close-first":
                probe.command("close")
                self.phase(daemon, probe, path, "descriptor closed; mapping held")
                probe.command("unmap")
                self.phase(daemon, probe, path, "descriptor closed; mapping released")
            elif name == "unmap-first":
                probe.command("unmap")
                self.phase(daemon, probe, path, "mapping released; descriptor held")
                probe.command("close")
                self.phase(daemon, probe, path, "mapping released; descriptor closed")
            elif name == "after-msync":
                probe.command("msync")
                self.phase(daemon, probe, path, "explicit msync of generation 1")
                probe.command("store 2")
                self.phase(daemon, probe, path, "later store after msync; both held")
                probe.command("close")
                probe.command("unmap")
                self.phase(daemon, probe, path, "later store after msync; both released")
            else:
                self.phase(daemon, probe, path, "descriptor and mapping held")
                probe.command("store 2")
                self.phase(daemon, probe, path, "later store; descriptor and mapping held")
        except (OSError, ValueError, KeyError, TypeError, http.client.HTTPException,
                subprocess.SubprocessError, HarnessError) as error:
            self.errors += 1
            self.emit("error", case=name, result="ERROR", error=str(error), exception=type(error).__name__)
        finally:
            if probe:
                try:
                    probe.close()
                except (OSError, subprocess.SubprocessError, HarnessError) as error:
                    self.errors += 1
                    self.emit("error", case=name, operation="probe cleanup", result="ERROR", error=str(error))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--mount", type=Path, required=True)
    parser.add_argument("--drive", required=True)
    parser.add_argument("--void", type=Path, required=True)
    parser.add_argument("--state-dir", type=Path, required=True)
    parser.add_argument("--probe", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--case", choices=CASES, help="default: run every case")
    args = parser.parse_args()
    for field in ("mount", "void", "state_dir", "probe", "out"):
        setattr(args, field, getattr(args, field).resolve())
    args.out.parent.mkdir(parents=True, exist_ok=True)
    with args.out.open("x", encoding="utf-8") as output:
        run = Runner(args, output)
        daemon = None
        run.emit("run", root=str(run.root), mount=str(args.mount), drive=args.drive,
                 state_dir=str(args.state_dir), probe=str(args.probe), void=str(args.void),
                 cases=[args.case] if args.case else list(CASES), sample_times=list(SAMPLE_TIMES),
                 final_sample_tolerance_seconds=FINAL_SAMPLE_TOLERANCE,
                 operation_timeout_seconds=TIMEOUT, implicit_sync=False, new_mount_opens_by_observer=False)
        try:
            if not args.mount.is_dir():
                raise HarnessError("--mount must be an existing test mount directory")
            for executable in (args.void, args.probe):
                if not executable.is_file() or not os.access(executable, os.X_OK):
                    raise HarnessError("executable is missing or not executable: " + str(executable))
            daemon = Daemon(run)
            run.root.mkdir()
            for name in ([args.case] if args.case else CASES):
                run.case(daemon, name)
        except (OSError, ValueError, KeyError, TypeError, http.client.HTTPException,
                subprocess.SubprocessError, HarnessError) as error:
            run.errors += 1
            run.emit("error", result="ERROR", error=str(error), exception=type(error).__name__)
        finally:
            if daemon:
                try:
                    daemon.close()
                except (OSError, ValueError, http.client.HTTPException, HarnessError) as error:
                    run.errors += 1
                    run.emit("error", operation="release read-only daemon session", result="ERROR", error=str(error))
        code = 2 if run.errors else 1 if run.failures else 0
        run.emit("summary", result="ERROR" if run.errors else "FAIL" if run.failures else "PASS",
                 failures=run.failures, errors=run.errors, root=str(run.root), exit_code=code,
                 elapsed_seconds=(time.monotonic_ns() - run.started) / 1e9)
    return code


if __name__ == "__main__":
    sys.exit(main())
