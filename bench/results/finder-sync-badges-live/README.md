# Finder Sync badges in a live Finder, 16 October 2026

Step 5, item 8. [The Finder Sync slice](../finder-sync-badges/README.md) was checked only in
process, against the agent's badge listener. This run enabled the extension, mounted a new drive
through FSKit and looked at it in Finder: the badges for each state, the context menu's Pause and
Resume Uploads, and `badges-check` against the installed agent. The user approved the
registration changes first. macOS 27.0.1 (26A434), Xcode 27.0 (27A5194q), Apple M5 Pro, branch
`finder-sync-badges` at `88b5d79` ([versions](raw/versions.txt)).

## Setup

- **Isolated rig**, as in
  [the matrix README](../mac-app-matrix/README.md#preparing-and-restoring-the-isolated-rig). A
  `voidfs-server` filesystem store listened on `127.0.0.1:9341`, with `--admin-listen 127.0.0.1:9342`.
  A test daemon had its own `VOIDFS_STATE_DIR` under `/private/tmp/vfl.PLNHSi`, and the drive was
  new: `badges-live-1792169905` ([daemon status](raw/daemon-status.json), [drive](raw/drive.json)).
  `~/Library/Application Support/voidfs` was absent before and after.
- **Selecting the dev build** ([register.txt](raw/register.txt)):
  - `VoidfsStateDirectory` set to the test state;
  - the `/Applications` agent unregistered and the dev build's agent registered;
  - `lsregister -u /Applications/voidfs.app`, then `pluginkit -a` on the dev build's module;
  - `pluginkit -e use -i dev.voidfs.app.findersync`. No System Settings step was needed.

  The agent ran from this build ([agent binary](raw/agent-binary.txt)). It logged
  `bridge serving HAUTK68F56.dev.voidfs.agent and HAUTK68F56.dev.voidfs.badges` with the test
  daemon's socket. Finder started four `VoidfsFinder` instances from this build's appex, and each
  connected to `HAUTK68F56.dev.voidfs.badges` ([processes](raw/processes.txt),
  [log](raw/log-stream-excerpt.txt)).
- **FSKit's module copy.** FSKit picked the module from the other `finder-sync-badges` worktree's
  build (`compassionate-payne-edbb8f`), not this one ([during](raw/registrations-during.txt)). Both
  builds are from commit `88b5d79`, and the PR doesn't change the module.
- **Mount.** `void mount <drive> <run>/mnt` had the daemon mount the drive with the `fskit` adapter,
  on the first try, so it is in the daemon's mount table, where the extension takes its scope from
  ([mount.txt](raw/mount.txt)).
- **Finder.** Computer use drove Finder: list and icon views of `live/`, a second window on the drive
  root, and right-clicks for the menu. Screenshots of the whole screen are cropped to the Finder
  windows.

## Results

Every state showed in Finder, on files and folders. The menu paused and resumed the drive.

| State | How it was made | Finder showed |
| --- | --- | --- |
| saved | `saved.txt` and `conflict.txt` written, then uploaded | green check ([01](raw/01-saved-zoom.jpg)) |
| saving | uploads paused from Finder's menu, then `saving.txt` written and `conflict.txt` edited | blue arrow ([03](raw/03-paused-list-zoom.jpg), [04](raw/04-paused-icons-zoom.jpg)) |
| pending | an empty `pending.txt`, and a new folder `nested/` holding an empty `inner.txt`, while paused | grey clock on all three |
| error | `error.txt` written while paused; its entry cancelled with `void uploads cancel 9` ([log](raw/cancel-and-peer.txt)) | red cross, before and after the resume |
| conflict | the SDK's `mount_peer` put `live/conflict.txt` while this Mac's edit waited, then Resume Uploads from the menu | orange triangle ([06](raw/06-resumed-icons-zoom.jpg)) |

- **Folders.** While paused, `live` showed error at the drive root, the most pressing state below
  it ([03](raw/03-paused-root-folder-zoom.jpg)). `nested` showed pending. After the resume,
  `live` showed conflict ([06](raw/06-resumed-root-folder-zoom.jpg)), and `nested` showed saved.
  The daemon's `POST /v1/mounts/states` answers at those moments match each badge
  ([paused](raw/states-paused.txt), [resumed](raw/states-resumed.txt)).
- **The context menu.** Right-clicking a file on the mount offered
  **Pause Uploads to badges-live-1792169905** ([02](raw/02-menu-pause.jpg)), and nothing that
  cancels. Choosing it paused the drive: `/v1/status` listed it under `pausedDrives`
  ([status](raw/status-after-menu-pause.txt)), and the writes after it waited as `paused`
  ([uploads](raw/held-writes.txt)). The menu then offered **Resume Uploads to …**
  ([05](raw/05-menu-resume.jpg)). Choosing that released the queue. By the next screenshot, about 7 seconds later, Finder had
  redrawn both open windows without a refresh: saved, conflict and error
  ([06](raw/06-resumed-icons-and-root.jpg)). The status then showed no paused drives, one
  conflict, and conflict.txt's entries as `conflict: the drive changed`
  ([states-resumed](raw/states-resumed.txt)). After the pause, `pausedDrives` held both the
  drive's alias and its id.
- **`badges-check` against the installed agent:** `refused (NSCocoaErrorDomain 4097): ok`
  ([output](raw/badges-check.txt)). The agent's own log shows the refusal is its signature
  check, not a missing service: on `HAUTK68F56.dev.voidfs.badges` it set the requirement
  `identifier "dev.voidfs.app.findersync"` and then logged "Received message forbidden due to code
  signing requirement" ([log](raw/badges-check-log.txt)). `bridge-check` was refused the same way
  for the module's requirement.

## Restoration

The [restore log](raw/restore.txt) shows these steps:

1. Unmount with `void unmount` (which also forgets the mount), stop the test daemon, the server
   and the log stream.
2. `pluginkit -e default` and `pluginkit -r` the Finder extension, and unregister the dev agent.
3. `lsregister -u` the dev app, then `lsregister -f -R /Applications/voidfs.app` and
   `pluginkit -a` its module.
4. Register the `/Applications` agent and delete `VoidfsStateDirectory`.
5. Delete the run directory and this worktree's build products, so LaunchServices can't find them
   again.

[Before](raw/registrations-before.txt) and [after](raw/registrations-after.txt) match:

- Finder Sync's list: MacPacker and Space, with no voidfs extension;
- `dev.voidfs.app.fskit` is enabled from `/Applications`. It has a new UUID and timestamp from
  the re-registration;
- `agent-status` says enabled, and the `dev.voidfs.app` domain is empty;
- the LaunchServices voidfs paths are the same;
- there are no voidfs mounts, test processes or listeners on 9341/9342, and no shared daemon
  state.

One thing differs. Before the run, the `/Applications` agent's launchd job was `spawn failed`
(`OS_REASON_CODESIGNING`), as after the 15 October restore. After the run it is `uninitialized`
and never started. It wasn't started to check whether it now runs: it would connect to the shared
daemon location, which this run doesn't touch. That is the user's to try.

To free the mount, the restore closed every Finder window, not only the two test windows. Any of
the user's own Finder windows that were open, on other Spaces for example, were closed too.

## Not shown

- **The drive's own icon.** The volume's icon on the desktop had no badge
  ([07](raw/07-volume-icon-no-badge.jpg)), although the daemon answers the drive root's state
  (conflict). Folders inside the drive are badged. The Finder sidebar and the Get Info badge
  labels weren't looked at.
- **Saving while uploads run.** Saving was shown only with the drive paused, as the brief asked.
  An upload in flight on a running queue wasn't tried.
- **Request counts with a live mount.** The mount's own change feed polls the server, so the
  in-process count of no requests for badges ([finder-sync-badges](../finder-sync-badges/README.md))
  wasn't repeated here.
- **A daemon restart under a live Finder, an offline link, Finder beside the menu bar's transfers
  view** ([PR #74](https://github.com/vishnu597/voidfs/pull/74)), **and two Macs.**
- **The FSKit module from this build.** FSKit used the identical build from the other worktree
  (see Setup).

## Replay

Build as in [apps/macos/README.md](../../../apps/macos/README.md) and the Rust tools as in
[finder-sync-badges](../finder-sync-badges/README.md#replay). Start the server and a test daemon as
in the matrix README, on spare ports. Record the before state:

```bash
pluginkit -m -v -p com.apple.FinderSync; voidfs status; voidfs agent-status; defaults read dev.voidfs.app
```

Select the dev build with the commands in [register.txt](raw/register.txt). Then:

```bash
void mount <drive> <empty folder>
```

Make the states in a folder on the mount, as in [held-writes.txt](raw/held-writes.txt) and
[cancel-and-peer.txt](raw/cancel-and-peer.txt). Pause and resume from Finder's context menu
between them. Restore with the commands in [restore.txt](raw/restore.txt), adjusted to the
baseline you recorded.
