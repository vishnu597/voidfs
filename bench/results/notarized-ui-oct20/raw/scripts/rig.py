#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Disposable notarized UI helpers. Importing this module has no side effects.

setup starts only the local backend/proxy and seeds a throwaway drive. configure
explicitly redirects the bridge. Native sign-in and Login Items actions belong to
the operator. Cleanup restores the supported original login-off baseline and
removes only this new state directory's Keychain item, retaining unfinished data.
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


R = Path.home() / ".voidfs-ui-rig-oct20"
D = R / "state"
C = R / "cli-state"
MNT = R / "mnt"
drive = "ui-check-oct20"
APP = Path("/Applications/voidfs.app/Contents/MacOS/voidfs")
VOID = Path("/Applications/voidfs.app/Contents/MacOS/void")
SERVER = Path("/private/tmp/voidfs-external-eject-target/debug/voidfs-server")
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
            raise RuntimeError("installed CLI could not generate the private rig key")
    KEY.chmod(0o600)
    _key()
    if server is None:
        server = SERVER
    if server is None or not Path(server).is_file():
        raise RuntimeError("build a local voidfs-server or pass --server <path>")
    server = Path(server).resolve()
    shutil.copy2(server, R / "bin/voidfs-server")
    version = run([server, "--version"], check=False)
    import hashlib
    emit("server-provenance", source=server, sha256=hashlib.sha256(server.read_bytes()).hexdigest(),
         version=version.stdout.strip(), cli=run([VOID, "version"]).stdout.strip())
    (R / "bin/voidfs-server").chmod(0o700)
    mode()
    start_backend()
    start_proxy()
    shown = peer("drive", "show", drive, check=False)
    if shown.returncode:
        peer("drive", "create", drive)
    root_file = R / "seeds/baseline-root.txt"
    nested_file = R / "seeds/nested/baseline-nested.txt"
    root_file.write_text("baseline root for native UI\n", encoding="utf-8")
    nested_file.write_text("baseline nested for native UI\n", encoding="utf-8")
    uploaded = peer("upload", root_file, nested_file.parent, drive + ":/")
    emit("initialized", drive=drive, endpoint=ENDPOINT,
         baseline=["baseline-root.txt", "nested/baseline-nested.txt"],
         upload_stdout=uploaded.stdout)


def verify_rig():
    value = json.loads((R / "rig.json").read_text(encoding="utf-8"))
    if value != {"uid": os.getuid(), "drive": drive, "state": str(D)}:
        raise RuntimeError("rig marker does not match this user and disposable state")


def _app(args, directory=None):
    """Capture app output privately; callers emit only summaries or redacted text."""
    command = [str(APP), *args]
    if directory is not None:
        command += ["-VoidfsStateDirectory", str(directory)]
    return subprocess.run(command, env=safe_env(), timeout=75, text=True,
                          capture_output=True, check=False)


def _login_state(text):
    line = next((line for line in text.splitlines() if line.startswith("login item: ")), "")
    value = line.removeprefix("login item: ")
    if value.startswith("on:"):
        return "on"
    if value.startswith("off"):
        return "off"
    if value.startswith("needs approval"):
        return "needsApproval"
    return "unknown"


def auth_status():
    verify_rig()
    key = _key()
    result = _app(["server-key", "status"], D)
    first = result.stdout.splitlines()[0] if result.stdout.splitlines() else ""
    signed_in = first.startswith("signed in to ")
    value = {"exit": result.returncode, "signed_in": signed_in,
             "endpoint_matches": signed_in and first.startswith("signed in to " + ENDPOINT + " ("),
             "id_matches": signed_in and (" as " + key["accessKeyId"] + ", for ") in first,
             "directory_matches": first.endswith(", for " + str(D)),
             "login_state": _login_state(result.stdout),
             "daemon_json_absent": not (D / "daemon.json").exists()}
    # The status text contains the access-key id; retain the boolean comparisons
    # rather than writing either known or unexpected credential ids to evidence.
    emit("app-auth-status", **value)
    return value


def verify_sign_in():
    value = auth_status()
    if not all(value[name] for name in ("signed_in", "endpoint_matches", "id_matches", "directory_matches")) or value["exit"]:
        raise RuntimeError("native sign-in did not keep the expected key in this private account")
    return value


def login_status():
    verify_rig()
    result = _app(["login-item", "status"], D)
    value = {"exit": result.returncode, "state": _login_state(result.stdout),
             "stdout": sanitized(result.stdout), "stderr": sanitized(result.stderr)}
    emit("app-login-status", **value)
    return value


def configure():
    """Explicit bridge override only; native UI owns sign-in and login switches."""
    import hashlib
    verify_rig()
    previous = R / "previous-global.json"
    if previous.exists():
        value = json.loads(previous.read_text())
        current = run(["/usr/bin/defaults", "read", "dev.voidfs.app", "VoidfsStateDirectory"], check=False)
        if not value.get("applied") or current.returncode or current.stdout.strip() != str(D):
            raise RuntimeError("existing setup did not leave this rig's override active")
        return auth_status()
    original_key = _app(["server-key", "status"])
    original_login = _app(["login-item", "status"])
    if original_key.returncode or original_login.returncode:
        raise RuntimeError("cannot inspect the original app account and login state")
    # A globally running/approval-pending production login job cannot be isolated
    # safely by changing its state directory. This fixture supports the observed
    # login-off baseline and leaves other initial states untouched.
    if _login_state(original_login.stdout) != "off":
        raise RuntimeError("this fixture requires an initial login-off baseline; original state was left unchanged")
    private = auth_status()
    if private["exit"] or private["signed_in"] or not private["daemon_json_absent"]:
        raise RuntimeError("the new private account already has a key or credential file; preserve it")
    old = run(["/usr/bin/defaults", "read", "dev.voidfs.app", "VoidfsStateDirectory"], check=False)
    value = {"override_present": old.returncode == 0, "override_value": old.stdout.strip(),
             "login_state": "off", "original_account_status_sha256": hashlib.sha256(original_key.stdout.encode()).hexdigest(),
             "private_account_was_empty": True, "applied": False}
    _atomic_json(previous, value)
    run(["/usr/bin/defaults", "write", "dev.voidfs.app", "VoidfsStateDirectory", "-string", D])
    value["applied"] = True
    _atomic_json(previous, value)
    run(["/bin/launchctl", "kickstart", "-k", "gui/%s/dev.voidfs.agent" % os.getuid()])
    emit("bridge-configured", state=D, login_baseline="off", import_file=KEY)
    return {"state": str(D), "import_file": str(KEY), "endpoint": ENDPOINT,
            "menu_relaunch_required": True}


def wait_daemon():
    verify_sign_in()
    def ready():
        try:
            value = ctl("GET", "/v1/status", timeout=3)
        except (OSError, http.client.HTTPException):
            return None
        return value if value["daemon"]["stateDir"] == str(D) and value["connection"]["endpoint"] == ENDPOINT else None
    value = wait(ready, timeout=45, what="installed app's private login daemon")
    if (D / "daemon.json").exists():
        raise RuntimeError("credential file is present; this run should use the native Keychain path")
    emit("native-daemon-ready", status=value, info=ctl("GET", "/v1/info"),
         installed_cli=run([VOID, "version"]).stdout.strip())
    return sanitized(value)


def status():
    value = {"auth": auth_status(), "login": login_status()}
    try:
        value.update(status=ctl("GET", "/v1/status", timeout=3),
                     info=ctl("GET", "/v1/info", timeout=3),
                     mounts=ctl("GET", "/v1/mounts", timeout=3), entries=entries())
    except (OSError, http.client.HTTPException):
        value["daemon_running"] = False
    emit("status", **value)
    return sanitized(value)


def mount():
    wait_daemon()
    value = run([VOID, "mount", drive, MNT], timeout=75)
    emit("mounted", stdout=value.stdout, mounts=ctl("GET", "/v1/mounts"))
    return status()


def offline():
    verify_rig()
    stop_proxy()
    value = wait(lambda: (view if view["connection"]["link"] == "offline" else None)
                 if (view := ctl("GET", "/v1/status")) else None,
                 timeout=45, what="private daemon offline")
    emit("offline", status=value)
    return sanitized(value)


def online():
    verify_rig()
    mode()
    start_proxy()
    value = wait(lambda: (view if view["connection"]["link"] == "online" else None)
                 if (view := ctl("GET", "/v1/status")) else None,
                 timeout=45, what="private daemon online")
    emit("online", status=value)
    return sanitized(value)


def scan():
    """Report exact known credential matches by file, never matching values."""
    verify_rig()
    values = [value for name, value in _key().items()
              if name in ("accessKeyId", "secretAccessKey", "serverKey") and isinstance(value, str) and value]
    files, leaked = 0, []
    for path in evidence.rglob("*"):
        if not path.is_file() or path.suffix in (".pyc",):
            continue
        files += 1
        data = path.read_bytes()
        if any(value.encode() in data for value in values):
            leaked.append(str(path.relative_to(evidence)))
    result = {"files": files, "leaked_files": leaked}
    emit("credential-evidence-scan", **result)
    return result


def cleanup():
    import hashlib
    verify_rig()
    ready = R / "ui-ready.json"
    if ready.exists() and not json.loads(ready.read_text()).get("released"):
        raise RuntimeError("release the UI holder and wait for its exit before cleanup")
    rows = entries() if (D / "state.sqlite").exists() else []
    if any(row["state"] not in ("done", "cancelled") for row in rows):
        raise RuntimeError("unpublished or kept writes remain; recover and verify them before cleanup")
    previous = R / "previous-global.json"
    old = json.loads(previous.read_text()) if previous.exists() else None
    if old and old.get("applied"):
        current = run(["/usr/bin/defaults", "read", "dev.voidfs.app", "VoidfsStateDirectory"], check=False)
        if current.returncode or current.stdout.strip() != str(D):
            raise RuntimeError("bridge override changed outside the rig; preserve the rig and original account")
        try:
            view = ctl("GET", "/v1/info", timeout=3)
        except (OSError, http.client.HTTPException):
            view = None
        if view:
            if not view["restartSafe"] or view["journal"]["unpublished"]:
                raise RuntimeError("daemon is not restart-safe")
            mounts = ctl("GET", "/v1/mounts")
            if any(Path(item["mountpoint"]) != MNT for item in mounts["mounts"] + mounts["remembered"]):
                raise RuntimeError("unexpected mount in private state; preserve it")
            if mounts["mounts"] or mounts["remembered"]:
                run([VOID, "unmount", MNT], timeout=75)
        # The account was empty at configure. Delete only this state directory's
        # throwaway Keychain item, then restore the original global login-off state.
        result = _app(["server-key", "sign-out"], D)
        emit("private-keychain-sign-out", exit=result.returncode, stdout=sanitized(result.stdout))
        result = _app(["login-item", "off"], D)
        emit("restore-login-off", exit=result.returncode, stdout=sanitized(result.stdout))
        state = auth_status()
        if state["signed_in"] or login_status()["state"] != old["login_state"]:
            raise RuntimeError("private Keychain/login state did not restore; preserve the rig")
        stopped = run([VOID, "daemon", "stop"], timeout=75)
        emit("private-daemon-stopped", stdout=stopped.stdout)
        if old["override_present"]:
            run(["/usr/bin/defaults", "write", "dev.voidfs.app", "VoidfsStateDirectory", "-string", old["override_value"]])
        else:
            run(["/usr/bin/defaults", "delete", "dev.voidfs.app", "VoidfsStateDirectory"])
        run(["/bin/launchctl", "kickstart", "-k", "gui/%s/dev.voidfs.agent" % os.getuid()])
        original = _app(["server-key", "status"])
        same = original.returncode == 0 and hashlib.sha256(original.stdout.encode()).hexdigest() == old["original_account_status_sha256"]
        emit("original-account-restored", status_matches_baseline=same)
        if not same:
            raise RuntimeError("original account status differs from its baseline; preserve the rig for review")
    stop_proxy()
    stop_backend()
    for source in (R / "server.log", R / "proxy.log", D / "daemon.log"):
        if source.exists():
            (evidence / source.name).write_text(sanitized(source.read_text(errors="replace")), encoding="utf-8")
    checked = scan()
    if checked["leaked_files"]:
        raise RuntimeError("credential values are present in saved evidence; redact before deleting the private key")
    emit("restored", previous_override_present=bool(old and old["override_present"]),
         login_state="off" if old else "unchanged", entry_states=[row["state"] for row in rows])
    shutil.rmtree(R)
    return {"rig_removed": True, "original_account_preserved": True}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["setup", "startserver", "configure", "auth-status", "verify-sign-in",
                                           "login-status", "wait-daemon", "mount", "status", "offline", "online", "scan", "cleanup"])
    args = parser.parse_args()
    if args.command == "setup":
        init(SERVER)
        value = {"rig": str(R), "endpoint": ENDPOINT, "import_file": str(KEY)}
    elif args.command == "startserver":
        verify_rig()
        value = {"backend_pid": start_backend(), "proxy_pid": start_proxy()}
    else:
        value = globals()[args.command.replace("-", "_")]()
    print(json.dumps(sanitized(value), sort_keys=True))

