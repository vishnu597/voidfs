# Cancel on changes that wait for a conflict, 16 October 2026

Step 5, item 8. The [live run](../item8-live/README.md) found that the menu
offered Cancel on a mount's changes queued behind a conflict (its changes 25 and 26). The queue
leaves those as they are, so Cancel changed nothing, and the menu said "Nothing was cancelled".
The menu now hides Cancel on them, as it does on the conflict itself. macOS 27.0.1 (26A434),
Xcode 27.0 (27A5194q), Apple M5 Pro, branch `item8-unmount-cancel` (on `item8-live`).

## Setup

- The server was a `voidfs-server` filesystem store on `127.0.0.1:9371`, with
  `--new-pool-feature multi-object-versions` and the throwaway local key from
  [the matrix README](../mac-app-matrix/README.md).
- The daemon had its own `VOIDFS_STATE_DIR` under `/private/tmp/vcw.fEVXbc`, and the drive was a
  new one, `waits-1792198128`.
- The app was the Release build of this branch, pointed at the daemon with
  `-VoidfsStateDirectory <dir>`. Nothing was mounted, and no registration was changed by hand.
- `~/Library/Application Support/voidfs` was absent before and after.

[waits_scene.rs](raw/waits_scene.rs) made the queue through a daemon session, as a mount makes its
changes ([output](raw/scene.txt)):
1. It saved `conflict.txt`, paused the drive, and saved it again.
2. Another computer's save went through the SDK.
3. After the resume, the drive refused this Mac's save: a conflict (changes 3 and 4).
4. Two more saves of `conflict.txt` queued behind the conflict (changes 5 to 8).
5. It saved `free.bin` (change 9) under a 2 KiB/s limit.

A 6-file import was queued too, and `void uploads limit 256B` held what was left
([uploads before](raw/uploads-before.txt), taken before that limit).

## Results

- **The menu shows no Cancel on what waits for the conflict** ([drawing](raw/view-before.png)).
  `free.bin` and the import have Cancel; the conflict and the saves queued behind it don't.
- **`transfers-selftest --cancel` passes** ([output](raw/selftest-cancel.txt), 25 checks):
  - Cancelling change 5, which waits for the conflict, leaves it queued, and the menu says
    nothing was cancelled.
  - Cancel on `free.bin` keeps it on this Mac, not uploaded.
- **Break.** With the menu's `cancellable(_:)` returning the old per-item answer, the menu showed
  Cancel on changes 5 to 8 ([drawing](raw/view-break.png)). The self-test then cancelled change
  5 as an ordinary change, which stayed queued, and 4 checks failed ([output](raw/selftest-break-cancel.txt)).
  The fixed build passed again on the same queue ([output](raw/selftest-fixed-again.txt),
  [uploads after](raw/uploads-after.txt)).

The menu goes by the conflicts' keys: a change to the same file, or to something in a folder
in conflict. The queue goes by inode, so a file deleted and made again under a conflict's name
would show no Cancel, though a cancel would keep it. A limited list holds every conflict whenever it
holds a queued change, since it lists failures first.

## Replay

```bash
cargo build --locked -p voidfs-cli -p voidfs-server
```

```bash
xcodebuild -project apps/macos/voidfs.xcodeproj -scheme voidfs -configuration Release -derivedDataPath apps/macos/build -allowProvisioningUpdates build
```

Start a test server and a test daemon on spare ports, as in
[the matrix README](../mac-app-matrix/README.md#preparing-and-restoring-the-isolated-rig) but
without mounting, and create a drive. Copy `raw/waits_scene.rs` to
`crates/voidfs-daemon/examples/`, then, with that environment:

```bash
cargo run --locked -p voidfs-daemon --example waits_scene -- "$VOIDFS_STATE_DIR/daemon.sock" <drive>
```

```bash
target/debug/void upload --detach <a folder of a few files> <drive>:/imported/
```

```bash
apps/macos/build/Build/Products/Release/voidfs.app/Contents/MacOS/voidfs transfers-selftest <drive> --cancel --png view.png -VoidfsStateDirectory "$VOIDFS_STATE_DIR"
```

`--cancel` cancels the newest unfinished import and clears finished uploads, which can't be
undone. Don't use it against a daemon whose imports you want to keep.
