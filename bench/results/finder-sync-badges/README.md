# Finder Sync badges, 16 October 2026

Step 5, item 8, the Finder Sync slice. A Finder Sync extension (`VoidfsFinder.appex`, in the app's
`Contents/PlugIns`) badges the items of mounted drives as saved, pending, saving, error or
conflict. It also offers a mounted drive's pause or resume in the context menu. The daemon is the
only source of state. macOS 27.0.1 (26A434), Xcode 27.0 (27A5194q), Apple M5 Pro, from `main` at
`38db05a` plus this branch ([versions](raw/versions.txt)).

## How it works

- **The daemon's call.** `POST /v1/mounts/states {drive, folder, after?, limit?, since?}` returns
  one folder's entries that aren't saved, each with its state, plus the folder's own state. A
  folder's state counts everything below it, most pressing first: conflict, error, saving, then
  pending. The daemon reads only its own SQLite state, in one read transaction: the folder's
  unsaved children by name, then a walk up from each unsaved inode of the drive (an index holds
  only those). No listing, attribute request or other call reaches the server, however many files
  the folder has. Pages hold 1,024 entries or half the daemon's response bound. While a first
  page's `version` still holds, `since` answers `unchanged` at once. The version is made of the
  daemon run, the drive session, the drive's metadata generation and the folder's listing
  generation. The call answers for a drive that a mount or a session has open, and never opens
  one.
- **Every other name is saved.** Whatever isn't saved is a change made on this Mac, and those stay
  in the daemon's local view whether or not the folder is listed now. An earlier draft treated an
  unlisted folder as unknown. The feed's echo of a new folder's own publication unlists it, so
  that folder's badges went blank. The first rig run caught this, and the draft was dropped.
- **The bridge.** The sandboxed extension can't reach the daemon's socket. The app's launch agent
  answers it on a second Mach service, `HAUTK68F56.dev.voidfs.badges`, and accepts only the
  Finder extension's code signature (`identifier "dev.voidfs.app.findersync"`, team
  `HAUTK68F56`), as the FSKit bridge accepts only the module's. It is not a proxy. Its four calls
  take typed arguments, which it checks before building the daemon request: mounts (the
  daemon's `/v1/mounts`, with each drive's pause from `/v1/status`), one folder's states, and a
  drive's pause and resume. It allows 16 calls in flight per connection. Pause and resume apply
  only to a drive mounted now.
- **The extension.** Its folder scope is the daemon's mounted mountpoints, polled every 3 seconds,
  including their `/private` spellings. When Finder asks for an item's badge, the extension reads
  the item's folder in one call and answers every item in it from that read. Folders Finder is
  showing, and folders it asked about in the last 30 seconds, are read again each second with
  `since`, so an unchanged folder costs one short local answer. A badge is sent to Finder only
  when it changes. A folder the daemon can't find, or a daemon that doesn't answer, shows no
  badges. The context menu has only Pause Uploads or Resume Uploads for the item's drive. It
  never offers cancel, because cancelling a mount's change marks the file's local copy as an
  error and deletes its staged bytes. That fate is still undecided.

## Setup

The rig was an isolated test daemon. It had its own `VOIDFS_STATE_DIR` under
`/private/tmp/claude-501/vfb` and a `voidfs-server` filesystem store on `127.0.0.1:9341`, with
`--admin-listen 127.0.0.1:9342` for `/metrics`. It used the throwaway local key from
[the matrix README](../mac-app-matrix/README.md) and new drives `badges` and `badges2`. Nothing was
mounted, and the FSKit module, the agent and the Finder extension were not registered or
enabled. `~/Library/Application Support/voidfs` was absent, and the `dev.voidfs.app` defaults were
empty, before and after the run.

Building the app made LaunchServices discover the build folder's Finder extension. `pluginkit`
listed it but did not enable it ([during the run](raw/registrations-during.txt)). It was
removed with `pluginkit -r` afterwards ([after](raw/registrations-after.txt)).

`voidfs badges-selftest` runs the extension's model, the same code the extension runs, against the
agent's badge listener, both in process. It runs the bridge's listener too, whose sessions make
the edits as a mount's would. Both listeners accept the app's own signature here. The check is
what the model told "Finder" for each item. A conflict comes from the SDK's `mount_peer`, which
saves the same file on the drive while this Mac's edit waits.

## Results

- **Each state, correct** ([output](raw/selftest-main.txt), 35 checks, no failures). Each run used
  a fresh folder. First, a saved file and the folder itself showed saved. With the drive's uploads
  paused:
  - an edited file showed saving, and a new empty file pending;
  - a file whose queued entry was cancelled showed error;
  - a new folder with a new file inside showed pending, as did that file;
  - the parent folder showed error, the most pressing state below it.

  After the resume, everything showed saved except the cancelled file (still error). The file
  the peer had changed showed conflict, and so did its folder. A folder that doesn't exist
  showed no badge.
- **Badges don't cost server requests.** With 200 saved files, the self-test first left the model
  stopped for 10 seconds, counting the server's `voidfs_s3_requests_total`. It then asked for all
  200 files' badges and polled the folders for 10 seconds. The server saw 0 requests in each
  window. The model made 34 calls for the 3 folders it read each second, and all 34 were answered
  `unchanged`. 200 badges were sent. The daemon test
  `badges_follow_the_queue_from_local_state_without_a_remote_request` shows the same at 1,000
  files ([Rust tests](raw/rust-tests.txt)). A counting proxy in front of the server carries the
  mount's listings, and then nothing while every badge call runs, the paged ones included.
- **Daemon restart** ([output](raw/selftest-restart.txt), 39 checks). `restart.sh` stops the test
  daemon when the self-test is ready and starts it again 3 seconds later. While no daemon
  answered, the badges cleared. Once a session opened the drive again, the conflict, error and
  saved badges and the folder's conflict came back from the daemon's state. A change still
  waiting through a restart (saving) is covered by the daemon test
  `badges_come_back_from_the_journal_after_the_daemon_restarts`.
- **Refused** ([output](raw/selftest-refused.txt)). With the badge listener requiring the Finder
  extension's signature, the app couldn't list the mounts, read a folder or pause a drive.
- **The agent's checks.** It refuses a pause from Finder for a drive that isn't mounted
  (`NotMounted`). It refuses a drive name with a slash, a folder with `..` and a cursor that isn't
  a name (`EINVAL`), a file as the folder (`ENOTDIR`) and a drive not open here (`NotOpen`).
- **The pictures** ([sheet](raw/badges.png)): saved, pending, saving, error and conflict, as SF
  Symbols on a white disc.

## Not shown

- **A live Finder, in this run.** The extension wasn't enabled and no FSKit volume was mounted
  here. A later run on the same day did both, with the user's approval:
  [finder-sync-badges-live](../finder-sync-badges-live/README.md) shows each badge in Finder, the
  context menu's pause and resume, and the installed agent refusing `badges-check`.
- **Finder and the menu bar side by side.** The menu's transfers view was in
  [PR #74](https://github.com/vishnu597/voidfs/pull/74), merged after this run; the two weren't run
  together.
- **Offline.** While the server can't be reached, the call still answers from local state (it
  never asks the server), but no run took the link down.
- **Deletions.** A pending deletion leaves no name in the folder, so it doesn't badge the folder.
  Moves badge where the local view shows the item now.
- **Two Macs.** A conflict made by the SDK peer on this Mac isn't two-Mac testing.

## Replay

```bash
xcodebuild -project apps/macos/voidfs.xcodeproj -scheme voidfs -configuration Release -derivedDataPath apps/macos/build -allowProvisioningUpdates build
```

```bash
cargo build --locked -p voidfs-cli -p voidfs-server && cargo build --locked -p voidfs-sdk --example mount_peer
```

Start a test server with `--admin-listen` and a test daemon on a spare port, as in
[the matrix README](../mac-app-matrix/README.md#preparing-and-restoring-the-isolated-rig) but
without mounting. Create a drive, and then, with that environment:

```bash
apps/macos/build/Build/Products/Release/voidfs.app/Contents/MacOS/voidfs badges-selftest <drive> main --socket "$VOIDFS_STATE_DIR/daemon.sock" --peer "$PWD/target/debug/examples/mount_peer" --metrics http://127.0.0.1:<admin port>/metrics --png badges.png
```

```bash
apps/macos/build/Build/Products/Release/voidfs.app/Contents/MacOS/voidfs badges-selftest <drive> refused --socket "$VOIDFS_STATE_DIR/daemon.sock"
```

```bash
bench/results/finder-sync-badges/restart.sh <run directory with env.sh> <drive> restart.txt
```

```bash
cargo test --locked -p voidfs-client --lib mount::states && cargo test --locked -p voidfs-daemon --test states
```

Each run leaves its folder on the drive. Its error and conflict files keep those states until
someone resolves them.
