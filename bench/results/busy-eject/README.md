# Busy eject, 19 October 2026

Normal eject refuses a busy FSKit volume. Force is an explicit choice: `void unmount
<drive-or-mountpoint> --force`, or the menu's **More actions → Force Eject…** and its
confirmation. Neither a busy normal eject nor an unsupported force action retries with a
different policy. Eject does not pause or cancel uploads. A successful daemon eject forgets
the mount, so restoring it requires an explicit mount.

The daemon control API accepts `POST /v1/mounts/unmount` with `target` and optional `force`
(default `false`, preserving existing callers). Only adapters implementing force support may
perform it. The FSKit adapter checks the filesystem type and resource at the mountpoint
before either form of eject; a replacement volume is left alone.

## Live setup

macOS 27.0.1, arm64, Xcode 27 beta. The daemon and server were built from `03e804c` in the
fresh `/private/tmp/voidfs-busy-eject-target`; the Swift app sources were compiled as a
standalone executable with Swift 6.4 in Swift 5 mode. The installed, previously notarized
`/Applications/voidfs.app` from `f5f6bd1` supplied the FSKit module and bridge. Its bundle and
registrations were unchanged. The new menu model was exercised by the standalone executable;
this was not another notarized app update.

The private disposable rig used a filesystem-backed server at loopback port 9373, a stoppable
proxy at 9372, drive `busy-eject-oct19` and a mount under `~/.voidfs-eject-rig-oct19`. Only the
bridge's temporary state-directory override changed. Start at login stayed off; no Keychain
item was added. A separate daemon-free peer talked directly to the backend. Credentials stayed
in a private file or child environment; copied logs redact their values.

## Results

All writes below were acknowledged by `write(2)` while the proxy was stopped. The original
payloads were kept outside the mount for byte comparisons.

| Busy condition | Normal eject | Explicit action | Payload recovered |
|---|---|---|---|
| Writable file descriptor held open | CLI, diskutil and menu model refused; mounted and remembered | CLI `--force`, 0.074 s | 262,177 bytes |
| Another writable file descriptor held open | CLI, diskutil and menu model refused; mounted and remembered | Menu model `force: true`, 0.151 s | 262,177 bytes |
| Another process's working directory in the volume; write descriptor closed | CLI and diskutil refused; mounted and remembered | `diskutil unmount force`, 0.237 s | 98,357 bytes |
| Writable file descriptor held open; native Finder Computer view | Eject button and Eject menu left the volume mounted, without a dialog | After the descriptor closed, the same normal Eject button removed it | 131,111 bytes |

For **each of the four payloads**, an explicit offline remount had a different kernel device
number and read exactly the acknowledged bytes. A subsequent offline daemon restart changed
both the PID and mount device; the bytes still matched and the entry stayed queued. Bringing
the proxy back uploaded the save without any resume command. The independent peer downloaded
matching bytes and SHA-256. The two old file descriptors tested after force returned `EIO`;
applications must reopen files after force eject. Normal daemon eject after closing handles
also passed.

Native Finder actions used the computer-use tool, with independent OS mount-table checks
([observations](raw/finder-observation.md)). Finder offered
no force dialog in this run. The new app confirmation's buttons were checked in source and
compiled, but were not clicked in a running new app; its normal and force model calls were
checked live through `transfers-selftest --eject-busy` and `--force-eject`.

## Regression checks

- The original multi-mount handler actually ejected an earlier volume, then a later busy
  refusal left both volumes remembered. The new test fails with that original handler and
  passes with the fix: every success is forgotten before trying the next mount.
- Seven adapter tests cover busy normal refusal without automatic force, forced failure
  retaining intent, forced success not restoring, replacement volumes, unsupported adapters,
  partial success and a SQLite failure after OS eject. That last case keeps the remembered
  row for retry without falsely reporting the gone volume as mounted.
- The CLI request-capture test checks normal `false` and explicit `true`, with no fallback.
  A full rerun caught macOS inheriting the mock listener's nonblocking flag. Explicitly
  making accepted streams blocking fixed it; ten focused repetitions passed. The failed run
  is retained as `raw/workspace-socket-race.txt`.
- Final full workspace: **785 passed, 0 failed, 9 ignored**, including the additional SQLite
  error-path test (`raw/workspace-tests-final.txt`). The initial run passed 784 tests before
  that last regression was added.
- Strict workspace Clippy, the warning-free full standalone Swift compile and all 56
  conformance cases / 432 steps passed. No S3 protocol or bucket-format behavior changed.

## Limits and restoration

An out-of-band Finder/diskutil eject still leaves the daemon's live/remembered intent stale
until `void unmount <mountpoint>` reconciles and forgets the already-gone volume. The run
records this before reconciliation. This change does not add mount-table event monitoring.

The resource check followed by path-based `umount` does not establish race safety for a future
privileged `/Volumes` helper. The pre-existing recovery of a dead FSKit mount on daemon startup
can still use `umount -f`; an ordinary eject request never escalates to force. Unsaved
application buffers and unsynchronized memory-mapped writes were not part of the acknowledged
`write(2)` checks. No crash/power-loss, logout/reboot, two-Mac, new notarization or `/Volumes`
helper test was performed here.

All four entries were published and byte-verified before cleanup. The test mount and remembered
intent, private daemon, proxy, server, credentials and rig were removed. The prior bridge
override was restored (absent), the bridge is enabled, Start at login is off, no voidfs mount
remains, and `~/Library/Application Support/voidfs` stayed absent. The installed app's deep,
strict code-signature verification passed after cleanup (`raw/restored.json`).

The canonical event record is [raw/rig.jsonl](raw/rig.jsonl). Commands and assertions are in
[raw/live-matrix.txt](raw/live-matrix.txt), [raw/finder-holder.txt](raw/finder-holder.txt) and
[raw/finder-recovery.txt](raw/finder-recovery.txt). Reproduction helpers and prerequisites are
in [raw/scripts/README.md](raw/scripts/README.md).
