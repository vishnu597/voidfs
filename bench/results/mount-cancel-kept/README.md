# Cancelling a mount's change: kept on this Mac, 16 October 2026

Step 5, item 8: the fate of a mount's cancelled change, as the user chose it on 16 October. Cancel
keeps the change on this Mac, not uploaded, and the next save uploads the whole file as it is
then, guarded on the drive's version. Cancel is offered in the menu bar only. macOS 27.0.1
(26A434), Xcode 27.0 (27A5194q), Apple M5 Pro, from `main` at `c8e37a1` plus this branch.
The design is in [step 5](../../../docs/step-5-macos.md#cancelling-a-mounts-change-16-october).

## Setup

**Before.** A Rust test, appended to `crates/voidfs-client/tests/mount_publication.rs` at `main`,
showed what cancel did until now ([test](raw/cancel-before.rs), [output](raw/cancel-before.txt)).
It isn't kept in the tree: its assertions are the old fate.

**After.** The new Rust tests, and the app's self-tests against an isolated test daemon:
- The daemon had its own `VOIDFS_STATE_DIR` under `/private/tmp/vi8.mdJayB`.
- The server was a `voidfs-server` filesystem store on `127.0.0.1:9361` (admin `127.0.0.1:9362`),
  with `--new-pool-feature multi-object-versions` and the throwaway local key from
  [the matrix README](../mac-app-matrix/README.md).
- The drive was a new one, `kept-1792180565`.
- The app was the Release build of this branch, pointed at the daemon with
  `-VoidfsStateDirectory <dir>`, which writes no preferences.
- Nothing was mounted or registered.
- `~/Library/Application Support/voidfs` was absent before and after.

## Results

- **Before: a cancelled change stranded its file** ([output](raw/cancel-before.txt)). The copy
  here kept the cancelled bytes and was marked `error`. The queue's frozen copy was deleted. A
  later save of the file failed for good ("the mount edit's predecessor was not published"),
  also after a resume and a restart. Another computer's save didn't show here and wasn't a
  conflict. Deleting the file here stayed queued and never reached the drive.
- **After: the Rust tests pass** ([output](raw/rust-tests.txt)). A cancelled change is `kept`:
  - Its frozen copy stays, through clear-finished and a restart.
  - The file reads as it is on this Mac and is marked `kept`.
  - A resume doesn't upload it.
  - The next save queues it again, and the drive gets the whole file.
  - Another computer's save in the meantime becomes a conflict at that save, with both
    versions kept here.
  - A new file's kept creation uploads as one put at its next save.
  - A kept edit followed by a deletion publishes the edit, then the deletion.
  - An upload stopped mid-flight by the cancel is kept too. When the next save queues it again,
    it goes alone, as an attempt that may have landed does, and the save follows it.
  - A cancel leaves a mount's conflict as it is, across a restart, and says it kept nothing.
  - The daemon's cancel counts what it kept, and the drive's other uploads are untouched.
  - Badges come back from the journal after the daemon restarts.
- **Finder's model** ([output](raw/badges-selftest.txt)). The badges self-test cancels
  `kept.txt` while the drive is paused, and its entry becomes `kept`. Finder's model then
  badges the file "kept", and its folder "kept" too, which outranks saving and pending. After
  the resume, the folder is "conflict" (from `mount_peer`'s save), and `kept.txt` stays "kept".
  35 checks, no failures. The [badge art](raw/badges.png) adds a purple minus, "Not uploaded:
  kept on this Mac".
- **The menu's model** ([output](raw/transfers-selftest-cancel.txt)). A 2 KiB/s limit held the
  mount changes made by `bridge-selftest` ([output](raw/bridge-selftest.txt)) and an 8-file
  import in the queue ([before](raw/uploads-before.txt)). Then `transfers-selftest --cancel`
  ran, with 23 checks, no failures:
  - The import's unfinished items were cancelled, and clear finished forgot the import.
  - The conflict left by the badges self-test has no Cancel button. Cancelling it anyway left it
    a conflict, and the menu said nothing was cancelled.
  - Cancelling the 9 MiB `bridge-….bin` creation kept it and the 8 later changes that waited
    for it (its rename, the moves into its folder, the folder).
  - The menu said "9 changes stay on this Mac, not uploaded: saving the file again uploads it
    as it is then."
  - The uploads line gained "4 files kept on this Mac, not uploaded".
  - The unrelated `copied` file went on uploading.
- **The menu and the CLI agree** after the cancel ([`void uploads`](raw/uploads-after.txt),
  [`void status`](raw/status-after.txt), [`voidfs transfers`](raw/transfers-after.txt)). The
  [drawing](raw/view-after-cancel.png) shows Cancel only on the uploading and queued changes,
  the kept count, and the notice. [Before the cancel](raw/view-before.png), the same queue has
  the import in it, Cancel on every waiting change, and only `kept.txt` kept.

## Not shown

- **Cancel clicked in the real menu.** The controls were called through the model the buttons
  use, and the view was drawn offscreen. A live FSKit mount follows in item 8's live session.
- **Two Macs.** The other computer was the SDK's `mount_peer` on this Mac.
- **An FSKit module older than this branch.** One can't read the new `kept` state from a newer
  agent. The app, the agent and the module ship in one bundle.

## Replay

```bash
cargo build --locked -p voidfs-cli -p voidfs-server && cargo build --locked -p voidfs-sdk --example mount_peer
```

```bash
xcodebuild -project apps/macos/voidfs.xcodeproj -scheme voidfs -configuration Release -derivedDataPath apps/macos/build -allowProvisioningUpdates build
```

```bash
cargo test --locked -p voidfs-client --test mount_publication -- a_cancelled_mount_change a_kept_
```

```bash
cargo test --locked -p voidfs-client --lib -- cancelling_a_mount_upload_in_flight mount_snapshot_is_frozen
```

```bash
cargo test --locked -p voidfs-daemon --test states --test uploads
```

Start a test server with `--admin-listen` and a test daemon on spare ports, as in
[the matrix README](../mac-app-matrix/README.md#preparing-and-restoring-the-isolated-rig) but
without mounting. Create a drive, and then, with that environment:

```bash
apps/macos/build/Build/Products/Release/voidfs.app/Contents/MacOS/voidfs badges-selftest <drive> main --socket "$VOIDFS_STATE_DIR/daemon.sock" --peer "$PWD/target/debug/examples/mount_peer" --metrics http://127.0.0.1:<admin port>/metrics --png badges.png
```

```bash
target/debug/void uploads limit 2KiB
```

```bash
apps/macos/build/Build/Products/Release/voidfs.app/Contents/MacOS/voidfs bridge-selftest <drive> main --socket "$VOIDFS_STATE_DIR/daemon.sock"
```

```bash
target/debug/void upload --detach <a folder of a few files> <drive>:/imported/
```

```bash
apps/macos/build/Build/Products/Release/voidfs.app/Contents/MacOS/voidfs transfers-selftest <drive> --cancel --png view.png -VoidfsStateDirectory "$VOIDFS_STATE_DIR"
```

`--cancel` cancels the newest unfinished import and clears finished uploads, which can't be
undone. Don't use it against a daemon whose imports you want to keep.
