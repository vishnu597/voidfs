# Menu-bar transfers, 16 October 2026

Step 5, item 8, first slice: the menu bar's transfers view. It shows the daemon's connection, upload
queue and mounts, and controls the queue: pause and resume (everything, a drive, an import or one
change), cancel (imports only), a bandwidth limit and clear finished. macOS 27.0.1 (26A434), Xcode
27.0 (27A5194q), Apple M5 Pro, from `main` at `38db05a` plus this branch
([versions](raw/versions.txt)).

The view reads the daemon's existing `/v1/status`, `/v1/uploads` and `/v1/uploads/watch`. It sends
the same pause, resume, cancel, limit, clear and unmount calls as `void uploads` and `void unmount`.
The app keeps no state of its own. A mount's changes can be paused and resumed but not cancelled
in the menu: cancelling one marks the file's copy on this Mac as an error and deletes the bytes
staged for it ([`Queue::cancel`](../../../crates/voidfs-client/src/queue.rs) and
`mount::publication::reconcile`), and the menu doesn't explain that yet.

## Setup

An isolated test daemon: its own `VOIDFS_STATE_DIR` under `/private/tmp/claude-501/vm8`, a
`voidfs-server` filesystem store on `127.0.0.1:9310` with the throwaway local key from
[the matrix README](../mac-app-matrix/README.md), and a new drive `menu-test`. The app was pointed at
it with the `-VoidfsStateDirectory <dir>` launch argument, which writes no preferences.
`~/Library/Application Support/voidfs` was absent before and after. The FSKit module and the
agent were not registered or changed. Imports were slowed with `void uploads limit` (2 KiB/s to
256 KiB/s) so that they stayed in progress.

A mount's changes came from `voidfs bridge-selftest menu-test main`. It edits the drive through
daemon sessions, without FSKit: 44 checks, no failures ([output](raw/bridge-selftest.txt)). Those
changes queue as a mount's do, with no batch and with the drive's stable ID.

## Results

- **The menu and the CLI agree.** `voidfs transfers` prints the menu's lines as `void status`
  lays out its rows. Its server and uploads lines are the same as `void status`'s, and its changes
  and imports match `void uploads`. Rates differ only by when each was sampled
  ([side by side](raw/status-and-transfers.txt)).
- **The controls work.** `voidfs transfers-selftest menu-test --cancel` calls each control through
  the model the buttons use, then checks the daemon's status or queue. All 15 checks pass
  ([output](raw/selftest.txt)): the limit is set, cleared and restored; pause and resume work for
  everything, for the drive, for an import and for a mount's change; cancelling the newest
  import cancels its unfinished items and leaves the mount's changes queued; clear finished
  forgets the import.
- **The view.** The self-test's `--png` draws the view offscreen, controls included. With imports
  in flight ([drawing](raw/view-imports.png)), each import and its changes have Pause and
  Cancel. With a mount's changes queued ([drawing](raw/view-mount-changes.png)), those changes
  have Pause only, under "11 changes are saved on this Mac and not yet on the drive." Rows past
  six changes and four imports are left to `void uploads`.
- **Restarts.** With the test daemon stopped, `voidfs transfers` says the daemon isn't running and
  exits 1. After `void daemon start` it shows the same 11 queued changes again, read from the
  journal. The menu's watch reconnects every 2 seconds, and its status refreshes every 2 seconds.

## Not shown

- **Clicks in the real menu.** Computer use can't reach a development app run from a build folder,
  so the drawings come from the offscreen view, and the controls were called through the model
  rather than clicked.
- **A live FSKit mount.** No mount was listed, so the mount rows (Pause, Resume and Unmount) were
  not drawn or pressed. A mount's changes were tested through daemon sessions.
- **Offline and degraded links,** the bridge restarting, and a large import (10,000 files) as the
  watch's one-a-second document.
- **Finder Sync badges,** which are not built, and a defined fate for a mount's cancelled save.
- **What waits while no daemon runs.** `void status` reads the journal ("11 changes (9.0 MiB)
  wait in the journal"); the menu only says the daemon isn't running.

## Replay

```bash
xcodebuild -project apps/macos/voidfs.xcodeproj -scheme voidfs -configuration Release -derivedDataPath apps/macos/build -allowProvisioningUpdates build
```

With a test daemon running, as in [the matrix README](../mac-app-matrix/README.md#preparing-and-restoring-the-isolated-rig) but
without mounting, and an import in progress:

```bash
apps/macos/build/Build/Products/Release/voidfs.app/Contents/MacOS/voidfs transfers -VoidfsStateDirectory "$VOIDFS_STATE_DIR"
```

```bash
apps/macos/build/Build/Products/Release/voidfs.app/Contents/MacOS/voidfs transfers-selftest <drive> --png view.png -VoidfsStateDirectory "$VOIDFS_STATE_DIR"
```

`--cancel` also cancels the newest unfinished import and clears finished uploads; don't use it
against a daemon whose imports you want to keep.
