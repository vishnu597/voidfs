#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Disposable busy-eject helpers. Importing this module has no side effects.

Run commands explicitly; start temporarily redirects the installed bridge to the
private rig state and runs the development CLI daemon. It changes no login-item
or Keychain registrations. Cleanup refuses to discard unpublished changes.
"""

import argparse
import datetime
import http.client
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time


R = Path.home() / ".voidfs-eject-rig-oct19"
D = R / "state"
C = R / "cli-state"
MNT = R / "mnt"
drive = "busy-eject-oct19"
APP = Path("/Applications/voidfs.app/Contents/MacOS/voidfs")
VOID = Path("/private/tmp/voidfs-busy-eject-target/debug/void")
BACKEND_PORT = 9373
PROXY_PORT = 9372
BACKEND = "http://localhost:%d" % BACKEND_PORT
ENDPOINT = "http://localhost:%d" % PROXY_PORT
SCRIPT = Path(__file__).resolve()
evidence = SCRIPT.parents[1]
MODE = R / "mode.json"
KEY = R / "key.json"


def safe_env(state=D, extra=None):
    names = ("PATH", "HOME", "USER", "LOGNAME", "TMPDIR", "LANG", "LC_ALL", "SHELL")
    env = {name: os.environ[name] for name in names if name in os.environ}
    env["VOIDFS_STATE_DIR"] = str(state)
    if extra:
        env.update({name: str(value) for name, value in extra.items()})
    return env


def _key():
    value = json.loads(KEY.read_text(encoding="utf-8"))
    if not value.get("accessKeyId") or not value.get("secretAccessKey"):
        raise RuntimeError("the rig key file is missing required fields")
    return value


def sanitized(value):
    """Remove credential fields and known rig credential values from evidence."""
    if isinstance(value, dict):
        return {str(k): ("[redacted]" if any(term in str(k).lower() for term in
                ("secret", "authorization", "cookie", "serverkey", "accesskey"))
                else sanitized(v)) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [sanitized(v) for v in value]
    if isinstance(value, Path):
        return str(value)
    if isinstance(value, str):
        try:
            key = _key()
        except (OSError, ValueError, RuntimeError):
            key = {}
        for name in ("serverKey", "secretAccessKey", "accessKeyId"):
            secret = key.get(name)
            if secret:
                value = value.replace(secret, "[redacted]")
        return value
    return value


def emit(event, **data):
    record = {"time": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "event": event, **data}
    record = sanitized(record)
    path = evidence / "rig.jsonl"
    with path.open("a", encoding="utf-8") as output:
        output.write(json.dumps(record, sort_keys=True) + "\n")
    return record


def run(args, *, state=D, timeout=30, check=True, extra_env=None):
    """Run a bounded command, returning captured, sanitized stdout and stderr."""
    args = [str(arg) for arg in args]
    if any(arg.split("=", 1)[0] in ("--secret-access-key", "--access-key-id", "--key")
           for arg in args):
        raise ValueError("use a key file or environment, never credential arguments")
    try:
        result = subprocess.run(args, env=safe_env(state, extra_env), timeout=timeout,
                                text=True, capture_output=True, check=False)
    except subprocess.TimeoutExpired as error:
        raise TimeoutError("command timed out after %ss: %s" %
                           (timeout, sanitized(args))) from None
    result.stdout = sanitized(result.stdout)
    result.stderr = sanitized(result.stderr)
    if check and result.returncode:
        raise RuntimeError("command failed (%s): %s\n%s" %
                           (result.returncode, sanitized(args), result.stderr.strip()))
    return result


def peer(*args, timeout=60, check=True):
    """Foreground peer against the backend, with a separate CLI state directory."""
    if len(args) == 1 and isinstance(args[0], (list, tuple)):
        args = tuple(args[0])
    args = [str(arg) for arg in args]
    if not args or args[0] in ("daemon", "mount", "unmount", "mounts", "uploads", "status") or "--detach" in args:
        raise ValueError("peer only runs direct server commands; it never controls a daemon")
    if (C / "daemon.sock").exists():
        raise RuntimeError("peer CLI state unexpectedly has a daemon socket")
    return run([VOID, "--json", "--endpoint", BACKEND, "--key-file", KEY, *args],
               state=C, timeout=timeout, check=check)


class UnixHTTPConnection(http.client.HTTPConnection):
    def __init__(self, path, timeout=10):
        super().__init__("localhost", timeout=timeout)
        self.path = str(path)

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(self.timeout)
        self.sock.connect(self.path)


def ctl(method, path, body=None, *, state=D, timeout=10):
    payload = None if body is None else json.dumps(body).encode("utf-8")
    connection = UnixHTTPConnection(Path(state) / "daemon.sock", timeout)
    try:
        connection.request(method, path, body=payload, headers={"Content-Type": "application/json"})
        response = connection.getresponse()
        raw = response.read().decode("utf-8")
        value = json.loads(raw) if raw else None
        if not 200 <= response.status < 300:
            raise RuntimeError("daemon HTTP %s: %s" % (response.status, sanitized(value)))
        return sanitized(value)
    finally:
        connection.close()


def entries(*, state=D):
    """Read the live journal without creating or mutating its SQLite database."""
    database = Path(state) / "state.sqlite"
    with sqlite3.connect(database.resolve().as_uri() + "?mode=ro", uri=True, timeout=5) as connection:
        connection.row_factory = sqlite3.Row
        rows = connection.execute("SELECT id, drive, key, state, error, retry_at, source, op, "
                                  "base, to_key, version, paused, size, sent, mount, mount_ino "
                                  "FROM entries ORDER BY id").fetchall()
    return sanitized([dict(row) for row in rows])


def wait(predicate, *, timeout=30, interval=0.1, what="condition"):
    deadline = time.monotonic() + timeout
    while True:
        result = predicate()
        if result:
            return result
        if time.monotonic() >= deadline:
            raise TimeoutError("timed out waiting for " + what)
        time.sleep(min(interval, max(0, deadline - time.monotonic())))


def _atomic_json(path, value):
    path = Path(path)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile("w", dir=path.parent, prefix=path.name + ".", delete=False,
                                         encoding="utf-8") as output:
            temporary = Path(output.name)
            json.dump(value, output)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def mode(*, fail_mutations=False, hold_feed=False):
    if not isinstance(fail_mutations, bool) or not isinstance(hold_feed, bool):
        raise ValueError("fault mode flags must be booleans")
    value = {"fail_mutations": fail_mutations, "hold_feed": hold_feed}
    _atomic_json(MODE, value)
    emit("proxy-mode", **value)
    return value


def _identity(pid):
    result = subprocess.run(["/bin/ps", "-p", str(pid), "-o", "uid=", "-o", "lstart=", "-o", "command="],
                            capture_output=True, text=True, timeout=5, check=False)
    return result.stdout.strip() if result.returncode == 0 else None


def _port_open(port):
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=0.3):
            return True
    except OSError:
        return False


def _start(name, args, port, *, extra_env=None):
    record = R / (name + ".pid.json")
    if record.exists():
        previous = json.loads(record.read_text(encoding="utf-8"))
        if _identity(previous["pid"]) == previous["identity"]:
            return previous["pid"]
    if _port_open(port):
        raise RuntimeError("refusing to replace another listener on port %s" % port)
    log = R / ("server.log" if name == "backend" else "proxy.log")
    descriptor = os.open(log, os.O_CREAT | os.O_WRONLY | os.O_APPEND, 0o600)
    with os.fdopen(descriptor, "ab") as output:
        process = subprocess.Popen([str(arg) for arg in args], env=safe_env(C, extra_env),
                                   stdin=subprocess.DEVNULL, stdout=output, stderr=output,
                                   start_new_session=True)
    identity = _identity(process.pid)
    if not identity:
        raise RuntimeError(name + " exited before its process could be recorded")
    def ready():
        if process.poll() is not None:
            raise RuntimeError(name + " exited; inspect its private rig log")
        return _port_open(port)
    wait(ready, timeout=15, what=name + " loopback listener")
    # Apple's python3 launcher execs the framework's Python binary. Record its stable
    # identity after it binds, rather than the short-lived launcher's argv[0].
    identity = _identity(process.pid)
    if not identity:
        raise RuntimeError(name + " exited before its ready identity could be recorded")
    _atomic_json(record, {"pid": process.pid, "identity": identity, "uid": os.getuid()})
    emit(name + "-started", pid=process.pid, port=port)
    return process.pid


def _stop(name):
    record = R / (name + ".pid.json")
    if not record.exists():
        return False
    value = json.loads(record.read_text(encoding="utf-8"))
    if value.get("uid") != os.getuid() or _identity(value["pid"]) != value["identity"]:
        raise RuntimeError("refusing to stop an unverified or reused " + name + " PID")
    os.kill(value["pid"], signal.SIGTERM)
    try:
        wait(lambda: _identity(value["pid"]) != value["identity"], timeout=3, what=name + " exit")
    except TimeoutError:
        if _identity(value["pid"]) == value["identity"]:
            os.kill(value["pid"], signal.SIGKILL)
        wait(lambda: _identity(value["pid"]) != value["identity"], timeout=5, what=name + " killed exit")
    record.unlink()
    emit(name + "-stopped", pid=value["pid"])
    return True


def start_proxy():
    return _start("proxy", [sys.executable, SCRIPT.with_name("proxy.py"),
                           "--listen-port", PROXY_PORT, "--upstream-port", BACKEND_PORT,
                           "--mode-file", MODE], PROXY_PORT)


def stop_proxy():
    return _stop("proxy")


def start_backend():
    key = _key()
    env = {"VOIDFS_ACCESS_KEY_ID": key["accessKeyId"], "VOIDFS_SECRET_ACCESS_KEY": key["secretAccessKey"]}
    return _start("backend", [R / "bin/voidfs-server", "--store", "fs:" + str(R / "server-data"),
                             "--listen", "127.0.0.1:%d" % BACKEND_PORT,
                             "--new-pool-feature", "multi-object-versions"], BACKEND_PORT, extra_env=env)


def stop_backend():
    return _stop("backend")


def init(server=None):
    os.umask(0o077)
    marker = R / "rig.json"
    if R.exists() and not marker.exists() and any(R.iterdir()):
        raise RuntimeError("refusing to reuse an unmarked rig directory")
    for path in (R, D, C, MNT, R / "bin", R / "server-data", R / "seeds", R / "seeds/nested"):
        path.mkdir(parents=True, exist_ok=True, mode=0o700)
        path.chmod(0o700)
    if marker.exists():
        verify_rig()
    else:
        _atomic_json(marker, {"uid": os.getuid(), "drive": drive, "state": str(D)})
    if not KEY.exists():
        with KEY.open("x", encoding="utf-8") as output:
            result = subprocess.run([str(VOID), "keys", "generate", "--scope", "admin", "--format", "json"],
                                    env=safe_env(C), stdin=subprocess.DEVNULL, stdout=output,
                                    stderr=subprocess.PIPE, timeout=15, check=False)
        if result.returncode:
            KEY.unlink()
            raise RuntimeError("installed void could not generate the private rig key")
    KEY.chmod(0o600)
    _key()
    if server is None:
        candidates = [VOID.with_name("voidfs-server"), Path("/private/tmp/voidfs-oct19-target/debug/voidfs-server")]
        server = next((candidate for candidate in candidates if candidate.is_file()), None)
    if server is None or not Path(server).is_file():
        raise RuntimeError("build a local voidfs-server or pass --server <path>")
    server = Path(server).resolve()
    shutil.copy2(server, R / "bin/voidfs-server")
    version = run([server, "--version"], check=False)
    import hashlib
    emit("server-provenance", source=server, sha256=hashlib.sha256(server.read_bytes()).hexdigest(),
         version=version.stdout.strip(), cli=run([VOID, "version"]).stdout.strip(),
         fallback=server == Path("/private/tmp/voidfs-oct19-target/debug/voidfs-server"))
    (R / "bin/voidfs-server").chmod(0o700)
    mode()
    start_backend()
    start_proxy()
    shown = peer("drive", "show", drive, check=False)
    if shown.returncode:
        peer("drive", "create", drive)
    root_file = R / "seeds/baseline-root.txt"
    nested_file = R / "seeds/nested/baseline-nested.txt"
    root_file.write_text("baseline root for busy eject\n", encoding="utf-8")
    nested_file.write_text("baseline nested for busy eject\n", encoding="utf-8")
    uploaded = peer("upload", root_file, nested_file.parent, drive + ":/")
    emit("initialized", drive=drive, endpoint=ENDPOINT,
         baseline=["baseline-root.txt", "nested/baseline-nested.txt"],
         upload_stdout=uploaded.stdout)


def verify_rig():
    value = json.loads((R / "rig.json").read_text(encoding="utf-8"))
    if value != {"uid": os.getuid(), "drive": drive, "state": str(D)}:
        raise RuntimeError("rig marker does not match this user and disposable state")


def status():
    verify_rig()
    value = {"status": ctl("GET", "/v1/status"), "info": ctl("GET", "/v1/info"),
             "mounts": ctl("GET", "/v1/mounts"), "entries": entries()}
    emit("status", **value)
    return value


def start():
    """Use the development daemon through the installed app's existing bridge."""
    verify_rig()
    previous = R / "previous-default.json"
    if not previous.exists():
        result = run(["/usr/bin/defaults", "read", "dev.voidfs.app", "VoidfsStateDirectory"], check=False)
        _atomic_json(previous, {"present": result.returncode == 0, "value": result.stdout.strip()})
    run(["/usr/bin/defaults", "write", "dev.voidfs.app", "VoidfsStateDirectory", "-string", D])
    run(["/bin/launchctl", "kickstart", "-k", "gui/%s/dev.voidfs.agent" % os.getuid()])
    started = run([VOID, "--endpoint", ENDPOINT, "--key-file", KEY, "daemon", "start"], timeout=45)
    emit("private-daemon-started", stdout=started.stdout, cli=run([VOID, "version"]).stdout.strip())
    return status()


def mount():
    verify_rig()
    mounted = run([VOID, "mount", drive, MNT], timeout=75)
    emit("mounted", stdout=mounted.stdout, mounts=ctl("GET", "/v1/mounts"))
    return status()


def offline():
    verify_rig()
    stop_proxy()
    value = wait(lambda: (v if v["connection"]["link"] == "offline" else None)
                 if (v := ctl("GET", "/v1/status")) else None,
                 timeout=45, what="rig link offline")
    emit("offline", status=value)
    return value


def online():
    verify_rig()
    mode()
    start_proxy()
    value = wait(lambda: (v if v["connection"]["link"] == "online" else None)
                 if (v := ctl("GET", "/v1/status")) else None,
                 timeout=45, what="rig link online")
    emit("online", status=value)
    return value


def cleanup():
    """Ordinary eject only; preserve the rig if acknowledged changes still wait."""
    verify_rig()
    rows = entries() if (D / "state.sqlite").exists() else []
    if any(row["state"] not in ("done", "cancelled") for row in rows):
        raise RuntimeError("unpublished or kept changes remain; recover and verify them before cleanup")
    try:
        info = ctl("GET", "/v1/info")
    except (OSError, http.client.HTTPException):
        info = None
    if info is not None:
        if not info["restartSafe"] or info["journal"]["unpublished"]:
            raise RuntimeError("daemon is not restart-safe; preserve the rig")
        mounts = ctl("GET", "/v1/mounts")
        if any(Path(item["mountpoint"]) != MNT for item in mounts["mounts"] + mounts["remembered"]):
            raise RuntimeError("an unexpected mount is in the rig state; preserve it")
        if mounts["mounts"] or mounts["remembered"]:
            # A busy ordinary eject is an error. There is deliberately no force fallback.
            unmounted = run([VOID, "unmount", MNT], timeout=45)
            emit("cleanup-unmounted", stdout=unmounted.stdout)
        stopped = run([VOID, "daemon", "stop"], timeout=75)
        emit("cleanup-daemon-stopped", stdout=stopped.stdout)
    previous = R / "previous-default.json"
    if previous.exists():
        current = run(["/usr/bin/defaults", "read", "dev.voidfs.app", "VoidfsStateDirectory"], check=False)
        if current.returncode != 0 or current.stdout.strip() != str(D):
            raise RuntimeError("bridge state override changed outside this rig; refusing to overwrite it")
        old = json.loads(previous.read_text(encoding="utf-8"))
        if old["present"]:
            run(["/usr/bin/defaults", "write", "dev.voidfs.app", "VoidfsStateDirectory", "-string", old["value"]])
        else:
            run(["/usr/bin/defaults", "delete", "dev.voidfs.app", "VoidfsStateDirectory"])
        run(["/bin/launchctl", "kickstart", "-k", "gui/%s/dev.voidfs.agent" % os.getuid()])
    stop_proxy()
    stop_backend()
    for source in (R / "server.log", R / "proxy.log", D / "daemon.log"):
        if source.exists():
            (evidence / source.name).write_text(sanitized(source.read_text(errors="replace")), encoding="utf-8")
    restored_override = previous.exists()
    emit("restored", previous_override=json.loads(previous.read_text()) if restored_override else None,
         remaining_entries=rows)
    shutil.rmtree(R)
    return {"removed": str(R), "bridge_override_restored": restored_override}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["setup", "init", "start", "mount", "offline", "online", "status", "cleanup"])
    parser.add_argument("--server", type=Path, help="local server binary; otherwise prefer the fresh target, then the earlier target")
    args = parser.parse_args()
    if args.command in ("setup", "init"):
        init(args.server)
        result = {"rig": str(R), "drive": drive, "endpoint": ENDPOINT, "cli": str(VOID)}
    else:
        if args.server is not None:
            parser.error("--server is only used by setup")
        result = globals()[args.command]()
    print(json.dumps(sanitized(result), sort_keys=True))
