# Disposable external-eject reconciliation rig

Importing these modules does nothing. The rig uses
`~/.voidfs-reconcile-rig-oct19`, drive `external-eject-oct19`, and verified-free
loopback ports 9372 (proxy) and 9373 (backend). Both the CLI and default server
come from `/private/tmp/voidfs-external-eject-target/debug/`. `setup --server PATH`
can explicitly choose another local server; there is no silent target fallback.
Server source, SHA-256 and available version output are saved in `rig.jsonl`.

The installed FSKit module and bridge agent must already be enabled. From this
directory, run explicitly:

```bash
/usr/bin/python3 rig.py setup
/usr/bin/python3 rig.py start
/usr/bin/python3 rig.py mount
/usr/bin/python3 live-matrix.py --run
```

`start` saves and temporarily changes the app's `VoidfsStateDirectory`, restarts
its existing bridge, and starts the development CLI daemon with a private rig
key file. Login-item and Keychain registrations are not changed. The script
controller must run outside the mounted folder, with Finder windows for the
automated test volume closed.

The matrix performs two rows on offline acknowledged writes:

1. Normal `diskutil unmount` after the writer closes and its bytes are queued.
2. Explicit `diskutil unmount force` while the writer remains open.

After each external eject the daemon must clear both live and remembered mount
state automatically. The script never invokes CLI unmount to reconcile that
eject. A daemon restart must not remount the forgotten intent. An explicit
offline mount must read the exact saved bytes on a new kernel mount device;
another daemon restart while intentionally mounted must restore it and read
the same bytes. Reconnecting must upload automatically without resume, and
the separate backend peer must read matching bytes. CLI unmount is used only
after those checks, to end an intentionally remounted test row cleanly.

Reconciliation times start after `diskutil` command return, not at a separately
instrumented kernel unmount. The no-remount check includes a two-second grace
interval and an empty persistent remembered table as exposed by the daemon API;
it is not an unlimited-duration monitor. Journal observations open SQLite
read-only. Any failure retains the rig and staged payloads for inspection.

For a separate native Finder check after the matrix:

```bash
/usr/bin/python3 finder-check.py hold
```

The holder creates an offline acknowledged save and writes `finder-ready.json`
inside the private rig. While it holds the writer, use Finder's Computer view
to attempt normal eject of only `external-eject-oct19` and record the busy
refusal. Touch `~/.voidfs-reconcile-rig-oct19/finder-release` and wait for the
holder to exit, then perform normal Finder eject again. Confirm the OS volume
is gone before running:

```bash
/usr/bin/python3 finder-check.py recover
```

Recovery requires automatic daemon cleanup and runs the same no-remount,
offline exact-byte and automatic peer-publication checks. No Finder GUI
action is issued by the script; GUI timing and evidence belong to the operator.
The holder closes automatically after five minutes if not released, and that
timeout is a failed run, not a successful Finder observation.

Finally:

```bash
/usr/bin/python3 rig.py status
/usr/bin/python3 rig.py cleanup
```

Cleanup refuses unfinished or kept changes, checks restart-safe state, and
never falls back to forced eject. It restores the original bridge override,
stops only identity-verified private processes, saves sanitized logs and removes
the rig. Keys stay in private files or child environments, never argument values
or saved logs. The installed app and extension registrations remain in place.
