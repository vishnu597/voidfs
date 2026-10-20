# Disposable installed-app UI rig

Importing the scripts does nothing. They use `~/.voidfs-ui-rig-oct20`, drive
`ui-check-oct20`, and verified-free loopback ports 9372/9373. Both CLI and app are
the installed `/Applications/voidfs.app` copies. The backend is copied strictly
from `/private/tmp/voidfs-external-eject-target/debug/voidfs-server`; its source,
hash and available version output are recorded separately from the installed CLI.

Run from this directory after installation:

```bash
/usr/bin/python3 rig.py setup
/usr/bin/python3 rig.py configure
```

This run also prepared an env-format import fixture privately from the generated
JSON key. `setup` creates `key.json`; prepare `key.env` before the native import:

```bash
/usr/bin/python3 - <<'PY'
import json, os
from pathlib import Path
os.umask(0o077)
root = Path.home() / ".voidfs-ui-rig-oct20"
key = json.loads((root / "key.json").read_text())
with (root / "key.env").open("x") as output:
    for name, value in [("VOIDFS_ENDPOINT", "http://localhost:9372"),
                        ("VOIDFS_REGION", "us-east-1"),
                        ("VOIDFS_ACCESS_KEY_ID", key["accessKeyId"]),
                        ("VOIDFS_SECRET_ACCESS_KEY", key["secretAccessKey"])]:
        output.write(name + "=" + value + "\n")
PY
```

`setup` creates a private key file and starts only the backend/proxy, then seeds
the throwaway drive. It prints the key file's path, never its values. `configure`
snapshots the original state-directory override and login/account status, then
temporarily redirects the bridge and restarts that existing agent. This fixture
requires the original login job to be off and the new account to be empty; it
leaves other initial states untouched. The original Keychain account is separate
because the app stores one item per state directory.

The operator must relaunch the installed menu after configure, so its watcher
uses the private directory. An explicit `-VoidfsStateDirectory` launch argument
can enforce that scope, but the operator must close/relaunch that process after
cleanup too. No new app copy or extension registration is needed.

Use the actual running menu to open its sign-in form and choose **Import key
file…** with the privately prepared `~/.voidfs-ui-rig-oct20/key.env`. That fixture
contains the endpoint/region as well as the key, so import can fill the Server
field with `http://localhost:9372` too. The generated `key.json` contains only the
key fields; importing that JSON requires entering the server separately. Verify
the populated form after reopening the menu if the file panel closes it. The
operator owns sign-in and every Login Items toggle; the helpers never perform
these test actions automatically.

```bash
/usr/bin/python3 rig.py auth-status
/usr/bin/python3 rig.py verify-sign-in
/usr/bin/python3 rig.py login-status
```

These commands compare the saved endpoint/key privately and report booleans,
not key values. `login-status` records the app's actual on/off/needs-approval
wording. Before and after disabling/re-enabling it in System Settings, capture
this output and the corresponding UI. A state cannot be called tested unless
the app actually reports it. Restore System Settings' original permission state
manually before cleanup; a not-registered service's status does not expose that
underlying permission switch.

Once the native login job is on:

```bash
/usr/bin/python3 rig.py wait-daemon
/usr/bin/python3 rig.py mount
/usr/bin/python3 ui-eject.py hold
```

The holder writes 262,177 acknowledged bytes offline and retains its writer for
up to five minutes. In the actual menu choose Force Eject, click **Cancel**, then:

```bash
/usr/bin/python3 ui-eject.py cancel-check
```

The same kernel volume, remembered mount, open writer and exact bytes must
remain. Choose Force Eject again and confirm in the actual native dialog, then:

```bash
/usr/bin/python3 ui-eject.py force-check
```

This observes cleared live/remembered state and durable queued bytes while the
writer is still held. It sends no eject request itself. Touch
`~/.voidfs-ui-rig-oct20/ui-release`, wait for the holder process to exit, then:

```bash
/usr/bin/python3 ui-eject.py recover
```

Recovery checks no remount after a daemon restart, exact bytes after an explicit
offline remount, intended offline restoration after another restart, and later
automatic upload plus exact backend-peer bytes without resume. New mount devices
exclude old kernel pages. The helper's observations do not time the native click
or prove a dialog appeared; the operator captures that evidence. A holder timeout
closes its descriptor and fails the run rather than confirming the UI flow.

```bash
/usr/bin/python3 rig.py status
/usr/bin/python3 rig.py scan
/usr/bin/python3 rig.py cleanup
```

Cleanup refuses held writers and unfinished/kept data. It unmounts ordinarily,
removes only the private account that was initially empty, restores the login-off
baseline and original override, and checks the original account's status against
its fingerprint. It saves sanitized logs and scans evidence for exact known key
values before removing the private rig. It never overwrites a pre-existing
Keychain account. Close the test menu before cleanup and relaunch normally after
restoration, especially if it had a test-directory launch argument.
