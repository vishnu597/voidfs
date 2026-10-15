# A new file's xattrs on a reader's descriptor, 15 October 2026

The FSKit volume skipped the daemon for a file whose attributes said it had no xattrs. With the
file open, it took that flag from the item's open handle. The core's handle attributes keep
`has_xattrs` from the open, and while a file has a stage (unpublished staged bytes) every open binds
it from the stage's first write. Finder writes a copy's bytes before setting its attributes. So a
reader that opened the copy before the bytes published got a handle that said there were none, and
`flistxattr` and `fgetxattr` on that descriptor failed. The fix (#72) asks the inode's attributes,
which the daemon's `listxattr` and `getxattr` also read. This run measured both gates on one dev
mount.

## Results

`apps/macos/scripts/xattr-after-write.py` on the mount. Each round copied a file and read its
attributes on a read-only descriptor straight after.

| Gate | Mode | Rounds | Read back whole at once | Missing for |
|---|---|---|---|---|
| Old: the open handle's (`-o xattr-gate=handle`) | `--finder` | 3 | 0 of 3 | 1.1 s each (until the bytes published) |
| Old | syscalls | 3 | 3 of 3, all inconclusive | – |
| Fix: the inode's (default) | `--finder` | 5 | 5 of 5 | – |
| Fix | syscalls | 3 | 3 of 3, all inconclusive | – |

- **Old gate, a miss.** The reader's descriptor listed none, and both `fgetxattr` calls were
  `ENOATTR`. `listxattr` by path straight after, with no handle open, listed all three names.
- **Syscall rounds were inconclusive.** The kernel tags files this process creates with
  `com.apple.provenance` before their first open. The script's removal reached the module
  (`setxattr 41 com.apple.provenance policy 3`). The kernel set the tag again 7 ms later, before
  the write open (`raw/trace.ndjson.gz`, inode 41). So each file had an attribute before its first
  write, and the bug can't show then, as with `cp` in #71. On APFS the tag never went away.
- **The control.** On APFS, `--finder` read 2 of 2 rounds back whole at once.

Raw: `raw/old-gate-finder.jsonl`, `raw/old-gate-syscalls.jsonl`, `raw/fix-finder.jsonl`,
`raw/fix-syscalls.jsonl`, with `raw/*-times.txt`.

## What the module saw

The build carried one diagnostic, not committed (`raw/setup/diagnostic.diff`):

- `-o xattr-gate=handle` restores the old gate.
- A trace line, `xattr gate <inode> <gate> handle <open> hasXattrs <answer>`, for every xattr call.

Without the option, the code is #72's. `raw/trace-excerpts.txt`, from `raw/trace.ndjson.gz`, with
repeated lines collapsed:

```text
== old gate (-o xattr-gate=handle), Finder copy round 0, inode 12
12:10:36.060 write 12 @0+43008 staged in 0.002662541 seconds, size 43008
12:10:36.063 setxattr 12 com.apple.ResourceFork policy 0 4096 bytes
12:10:36.081 setxattr 12 user.voidfs.sentinel policy 0 26 bytes
12:10:36.109 close 12 writable in 0.019142417 seconds
12:10:36.121 xattr gate 12 handle handle false hasXattrs true
12:10:36.218 open 12 modes 1 cache 1 granted 1
12:10:36.219 xattr gate 12 handle handle true hasXattrs false
12:10:36.222 close 12 read in 0.00064225 seconds

== fix (default), Finder copy round 0, inode 48
12:11:38.591 close 48 writable in 0.018229125 seconds
12:11:38.714 open 48 modes 1 cache 1 granted 1
12:11:38.716 xattr gate 48 inode handle true hasXattrs true
12:11:38.733 close 48 read in 0.000643542 seconds
```

`bridge-selftest main`, in process against the same daemon, passed 44 of 44 checks, including the
two new ones (`raw/bridge-selftest.txt`). It printed `note: the reader's open bound hasXattrs
false`: a reader opening the file during staging gets a handle that says none, through the agent
as through the core.

The 1.1 s window here is the daemon's publication delay on a loopback server. #71's "about a minute"
was when it read the files again, not how long they were missing.

## Setup and restoration

The isolated rig of [the application matrix](../mac-app-matrix/README.md#preparing-and-restoring-the-isolated-rig):

- **Run directory.** `/private/tmp/vmr.WaUm15`, holding the state, the server store and the mount.
- **Server and drive.** `voidfs-server` on 127.0.0.1:9000 with the throwaway local key, and a new
  drive `xattrfix-1792080467`.
- **Daemon.** Its own state directory. `~/Library/Application Support/voidfs` was absent before and
  after.
- **Builds.** CLI and server: `cargo build --locked -p voidfs-cli -p voidfs-server`. App: the
  README's `xcodebuild` with `-derivedDataPath $SCRATCH/xcbuild` (`$SCRATCH` is the session's
  scratch folder), from #72's head plus the diagnostic.

`raw/setup/` has every step's commands and output, in order:

1. **Before:** `*-before.txt`. The `/Applications` agent was enabled and running, its extension
   enabled, no voidfs mounts, port 9000 free.
2. **Selecting the dev build:** `register.txt`. It sets `VoidfsStateDirectory`, unregisters the
   `/Applications` agent, registers the dev agent, runs `lsregister -u /Applications/voidfs.app`
   and `pluginkit -a` the dev extension. `voidfs status` then showed the dev extension enabled.
3. **The first mount** failed (`mount-old-gate.txt`): the dev agent's first start exited 78
   (`EX_CONFIG`). Unregistering and registering it again fixed that (`reregister-1.txt`), as
   [the app's README](../../../apps/macos/README.md#the-bridge-to-the-daemon) describes for a switch
   between copies.
4. **Mounts:** `mount-old-gate-2.txt`, then `remount-fix.txt`. Both used
   `mount -F -t voidfs -o trace[,xattr-gate=handle]`, and both were served by the dev extension and
   agent.
5. **Restoration:** `restore.txt`. It unmounts, stops the daemon, the server and the log stream,
   unregisters the dev agent, runs `lsregister -u` on the dev app and
   `lsregister -f -R /Applications/voidfs.app`, `pluginkit -a` the `/Applications` extension,
   registers the `/Applications` agent, and deletes `VoidfsStateDirectory`.
6. **After:** `after-restore.txt`. It matches the baseline:
   - the preferences domain is empty;
   - the `/Applications` extension is enabled;
   - `agent-status` says enabled;
   - the LaunchServices voidfs paths are those from before;
   - no mounts, test processes or listener on 9000;
   - no shared daemon state.

   One thing differs. The `/Applications` agent, running before, was refused at its first start
   (`OS_REASON_CODESIGNING`, validation category 3), and still was after three more
   unregister/register cycles (`reregister-restore.txt`). That is the Developer ID build's known
   launch-constraint refusal. Clearing it is left to the user, as after earlier runs: Login Items,
   or a log out and in. After #71's restore today the agent wasn't running either, and it was
   running again 48 s later. What started it isn't recorded.

## Commands

```bash
/usr/bin/python3 apps/macos/scripts/xattr-after-write.py "$MATRIX_MOUNT" --finder --rounds 3 --settle 120
```

```bash
/usr/bin/python3 apps/macos/scripts/xattr-after-write.py "$MATRIX_MOUNT" --rounds 3 --settle 120
```

```bash
"$APP/Contents/MacOS/voidfs" bridge-selftest "$MATRIX_DRIVE" main --socket "$VOIDFS_STATE_DIR/daemon.sock"
```
