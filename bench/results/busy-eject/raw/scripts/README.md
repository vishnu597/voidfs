# Disposable busy-eject rig

These helpers do nothing when imported. They use `~/.voidfs-eject-rig-oct19`, drive
`busy-eject-oct19`, and loopback ports 9372 (proxy) and 9373 (backend). The client is
`/private/tmp/voidfs-busy-eject-target/debug/void`. They require the installed FSKit
module and bridge agent to be enabled already.

Run from this directory, explicitly and in order:

```bash
/usr/bin/python3 rig.py setup
/usr/bin/python3 rig.py start
/usr/bin/python3 rig.py mount
/usr/bin/python3 rig.py status
```

`setup --server /absolute/path/to/voidfs-server` selects a server explicitly.
Without it, setup prefers the busy-eject target's server and falls back to
`/private/tmp/voidfs-oct19-target/debug/voidfs-server`. The event log records the
selected source path, SHA-256, version output and whether the fallback was used.
The fallback is an earlier local build, not a rebuild of the current checkout.

`start` temporarily sets the app's `VoidfsStateDirectory`, restarts the existing
bridge agent, and starts the development CLI daemon with a private key file.
It saves the previous override for cleanup. It does not change login-item or
Keychain registrations. `mount` remembers the disposable mount at the rig's `mnt`
folder. The fixture includes `baseline-root.txt` and `nested/baseline-nested.txt`.

```bash
/usr/bin/python3 rig.py offline
```

This stops only the identity-verified proxy and waits for the daemon to report
offline. The backend remains available to the separate peer. The live matrix can
use the imported `run`, `ctl`, `entries`, `peer`, `start_proxy`, `stop_proxy`, `mode`,
`wait`, and `emit` helpers. Hold busy descriptors in the controller process while
invoking eject in a child process. The controller must not use the mounted folder
as its own working directory unless that is the intended busy condition.

The automated matrix runs all three explicit force paths and verifies offline remount,
offline restart and later peer bytes:

```bash
/usr/bin/python3 live-matrix.py
```

It ends with no mount. `finder-check.py hold` then mounts a fourth offline save and holds
its descriptor for up to four minutes. In Finder's Computer view, select only the named
test volume's normal eject action, verify it remains in the OS mount table, then touch
the rig's `finder-release` file. After the holder exits, repeat normal Finder eject,
verify the OS volume is gone, and run `finder-check.py recover`. Use a fresh rig for a
new measurement; fixed payload names and prior Finder windows can affect a reused rig.

After the matrix has closed its descriptors, recover and verify acknowledged data:

```bash
/usr/bin/python3 rig.py online
/usr/bin/python3 rig.py status
```

Wait for the recorded journal rows to become `done`, and use `peer("show", drive,
key, "--output", destination)` to verify each original payload. The peer has its
own daemon-free state directory and talks directly to the backend. A queue state
check alone does not compare file bytes.

```bash
/usr/bin/python3 rig.py cleanup
```

Cleanup refuses to discard unfinished or kept changes, requires restart-safe
state, and uses ordinary eject with no force fallback. A busy eject error leaves
the rig available for inspection. On success it stops the private daemon,
restores the previous bridge override, restarts the bridge, stops the verified
proxy/backend, copies sanitized logs into this evidence folder and removes the
rig. The installed app and its extension registrations remain in place.

No timeout or process-identity match is permission to kill an unrelated process.
Ports must be free, and no other controller may change the bridge override during
the run. The helper refuses unexpected process identities and state overrides.
Keys stay in a private rig file or the backend's child environment; never print
that file or include its values in command arguments.
