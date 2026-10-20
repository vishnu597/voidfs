# External eject reconciliation, 19 October 2026

Finder and diskutil ejects now clear daemon-managed FSKit mount state automatically. The
daemon checks the OS mount table about once a second, independently of status requests or an
open menu. Confirmed absence or replacement retires only that mountpoint and deletes its
remembered intent. The watcher never calls `umount`, changes upload state or reads file data.

Probe failures, malformed output and timeouts mean unknown, so intent is retained. An
unreadable module whose exact resource is still mounted also retains intent. The watcher stops
before intentional daemon detach, preserving remembered mounts across orderly restart.

## Implementation and regressions

The adapter's `Mounted::is_present` defaults to retaining intent, allowing adapters to opt in.
FSKit checks its canonical mountpoint, filesystem type and exact `voidfs://` resource. Concurrent
checks share a 250 ms mount-table snapshot; queries have a 500 ms deadline. A successful mount
or adoption invalidates earlier snapshots before exposing the new handle. The workspace's
unsafe-code prohibition is unchanged.

Read-only probes run outside the mount-operation lock, with at most eight in flight and a
one-second outer deadline. Before retiring anything, the watcher acquires that lock and checks
the same handle incarnation still owns the path. A stale response cannot erase a remount.
Pending SQLite deletes retry even after the live table empties; a successfully remembered new
mount supersedes an old pending delete. Shutdown cancels probes and drains already-started
persistence before detaching volumes.

Sixteen integration tests cover external absence, APFS/other-drive replacements, per-mount
cleanup, probe errors and repeated timeouts, unreadable-but-mounted volumes, SQLite retry and
supersession, stale observations, orderly stop during a held probe, and queued-write recovery.
Three focused unit tests cover parsing, malformed complete snapshots and cached pre-mount
absence. Disabling the new watcher reproduces the stale-intent failure; the same regression
passes with it enabled (`raw/without-watcher.txt`, `raw/with-watcher.txt`). The disabled-watcher
run reproduces the prior behavior; it is not a checkout of the complete old source tree.

The full workspace passed **796 tests, 0 failures, 9 ignored** in the fresh target. Strict
workspace Clippy passed with warnings denied, as did all 56 conformance cases / 432 steps.
Python 3.9 parsed every reproduction helper. No Swift source changed in this slice.

## Live checks

macOS 27.0.1 (26A434), arm64. Development CLI/daemon product code `dc13cc6` was built in the fresh
`/private/tmp/voidfs-external-eject-target`. The installed, previously notarized `f5f6bd1` app
supplied its FSKit module and bridge. No app bundle, extension registration, login-item setting
or Keychain item changed. The bridge temporarily used the private rig's state directory.

The disposable drive `external-eject-oct19` mounted under `~/.voidfs-reconcile-rig-oct19`. A
stoppable loopback proxy at port 9372 connected to a filesystem-backed server at 9373. A separate
daemon-free peer accessed the backend directly. Every measured save was acknowledged while
the proxy was stopped, with the original bytes retained outside the mount.

| External eject | Offline payload | Automatic live + remembered cleanup |
|---|---:|---|
| Normal `diskutil unmount`, writer closed | 131,111 bytes | First observed 0.565 s after command return |
| Explicit `diskutil unmount force`, writer held open | 262,177 bytes | First observed 0.405 s after command return |
| Native Finder Computer-view Eject, after writer closed | 98,357 bytes | Already clear when recovery observed it; GUI latency not measured |

The diskutil numbers are single observations, sampled about every 50 ms after the command
returned. OS-table absence was also checked. Full checks including that subprocess completed
at 0.574 and 0.430 seconds. These are neither medians nor an upper latency guarantee.

For **each of the three saves**:

1. Successful external eject cleared both live and remembered state without a CLI reconciliation
   call. Restarting the daemon changed its PID and did not remount; repeated API and OS-table
   checks stayed empty through a two-second grace interval.
2. An explicit offline remount used a different kernel device number and read exactly the
   acknowledged bytes. An intentional offline restart then restored the remembered mount on
   another fresh device, with the same bytes and hashes.
3. Reconnecting uploaded the queued entry without resume. The independent peer downloaded
   identical bytes and SHA-256. The original entry ID survived the restarts.

The force-ejected old descriptor returned `EIO`. Native Finder's normal eject button left the
busy volume visible, mounted and remembered while the writer was held; after it closed, the same
button removed the OS volume and the watcher cleared intent. Both Finder observations include
independent OS mount-table checks in `raw/rig.jsonl`; the new Finder window was closed afterwards.
CLI unmount was used only to finish the deliberately remounted recovery rows, after automatic
reconciliation and no-remount checks had already passed.

## Limits and restoration

The daemon must be running to observe an external eject. It cannot infer an eject performed
while stopped, or one immediately followed by a daemon crash before its remembered intent was
deleted. It observes OS mount absence, not the identity or motive of the program that removed
the volume. A module/bridge failure that leaves the OS volume present does not forget it.

If SQLite refuses deletion, the OS volume is no longer reported mounted, but durable remembered
intent remains until deletion succeeds. Retrying is in memory; restarting during a persistent
storage failure can still restore that row. No tombstone is persisted outside the database.
The presence checks introduce no force policy or `/Volumes` helper. Startup's existing dead-module
recovery remains separate. Power loss, unsynchronized mmap stores, two Macs and version-history
cardinality were not tested. No new app was notarized or installed.

All three entries were uploaded and byte-verified before cleanup. The private daemon, server,
proxy, credentials, mount, remembered intent and rig were removed. The bridge override is absent,
the bridge is enabled, Start at login is off, no voidfs mount or listener remains, and the
default app-state directory stayed absent. The installed app's deep strict signature verification
passed (`raw/restored.json`). An exact-value scan of all saved evidence found no throwaway
credential values after sanitized logs were copied and the key file removed.

Commands and assertions are in [raw/live-matrix.txt](raw/live-matrix.txt),
[raw/finder-holder.txt](raw/finder-holder.txt), [raw/finder-recovery.txt](raw/finder-recovery.txt)
and the canonical [raw/rig.jsonl](raw/rig.jsonl). Reproduction instructions are in
[raw/scripts/README.md](raw/scripts/README.md). Workspace results, strict Clippy and conformance
validation are saved alongside them. No S3 protocol or bucket-format behavior changed.
