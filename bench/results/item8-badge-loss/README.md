# Finder's lost badges after the live run's restart, 16 October 2026

Step 5, item 8. The [live run](../item8-live/README.md) left one finding open: a Finder window on
`live/`, captured after the degraded (503) link recovered, showed no badge on any item, while the
daemon's `POST /v1/mounts/states` still reported `kept` and `conflict`. macOS 27.0.1 (26A434),
Xcode 27.0 (27A5194q), Apple M5 Pro, branch `item8-live` at `28cec07` plus this change
([versions](raw/versions.txt)).

## What happened

The window without badges was not a window that lost them. It was opened right after a daemon
restart that remounted the volume in 0.25 s, and it never showed a badge.

- **The window.** The screenshot ([r](../item8-live/raw/live/r-recovered-finder.jpg)) is of
  window 9910. Phase O opened it at 16:46:57.8, 1 s after restarting the daemon with the control
  fix. Screen capture failed during P and Q, and its first screenshot was at 16:49:27, after R.
  The README's "16:51" is when the file was copied. The badges of phases I to N were in another
  window, 9884, opened before the restart ([timeline](raw/timeline.md)).
- **The remount.** The system log has every unmount and mount of the run
  ([log](raw/logs/live-run-mounts.txt)). In phase O, fskitd unmounted the volume at
  16:46:56.477 and mounted it again at 16:46:56.731. Finder saw volume -504 go and volume -505
  come, and opened `live` as `.file/id=33114754.17`. Before O, the same folder was
  `.file/id=33059174.17`. Each mount gets a new file system id, and Finder knows items by it.
- **The model didn't notice.** The extension's model (`apps/macos/Badges/Badges.swift`) set
  Finder's `directoryURLs` only when the set of mounted mountpoints changed. It reads the
  daemon's mounts every 3 s, and a read lands in a 0.25 s gap roughly one time in twelve. Every
  read after the restart saw the same mountpoint, mounted, so Finder wasn't told the folder again.
- **Phase I is the comparison.** Phases H and I stopped the daemon for 9.8 s with the same
  extension processes running (pids 24784 to 24788, 16:37:17 to 16:52:40). The model's reads
  found no daemon, told Finder there were no folders, then the mountpoint again. A window opened
  1.7 s after that remount showed every badge ([i](../item8-live/raw/live/i-finder-after-daemon-restart.jpg)).
  Of the run's other remounts, five were rebuilds of the app, which started new extension
  processes that told Finder their folders afresh. The sixth, a 0.13 s `void unmount && void
  mount` in phase F, had no window opened on the mount before the next rebuild.

So Finder held the folder it was told before the remount, as the volume that was there then. It
asked the extension nothing about the new volume's items, and showed no badge at all, folders
included. That is an inference: Finder doesn't log its Finder Sync requests, and the extension
had no logging in that build. It fits every capture, and nothing else in the model does:

- **The 503 phase can't blank the model's badges.** The daemon's folder-states call reads only
  its own SQLite state, never the server. During Q it answered `kept` and `conflict` in 0.02 s
  ([q](../item8-live/raw/live/q-degraded.txt)).
- **Asked again, the model always answers.** Whatever Finder does after a failed listing, a
  request for an item in a folder the model has read is answered at once, unchanged or not. The
  model tells an item "no badge" only for a folder the daemon can't find, or a mount that's gone.
  Neither was so: the daemon found `live` throughout, and the mount stayed listed. Every item,
  folders included, had no badge, so Finder hadn't asked.

## The fix

The model now compares what the kernel has mounted at each mountpoint, not only the daemon's mount
table:

- `BadgeModel.volumes()` reads the kernel's table with `getfsstat(MNT_NOWAIT)`, which answers
  without asking any file system. A hung FSKit module can't hold it. It returns each
  mountpoint's file system id, which a new mount at the same place doesn't keep.
- With each read of the mounts, every 3 s, each mount carries the volume mounted there now.
- When a mountpoint's volume changes, the model does what a slow restart did. It tells Finder
  the folders without that mount, then with it, then tells it again the badges of the items it
  had asked about there.

A sandboxed command-line tool, ad hoc signed with only `com.apple.security.app-sandbox`, read the
kernel's twelve mounts with `getfsstat` like this. So the Finder extension, which has the same
sandbox, can too. The probe was a throwaway. Its container's data is deleted, but macOS refused
to delete the container itself, `~/Library/Containers/dev.voidfs.sbxprobe`, which holds only its
metadata file.

## The test

`voidfs badges-selftest <drive> remount [--degraded]` runs the extension's model against the
agent's badge listener in process, as the other modes do, and changes the volume the model reads at
the mount between two of its reads of the mounts. It checks:

1. A saved file, a change held by the drive's pause and their folder get their badges. Finder is
   told the mount's folder once.
2. With `--degraded`, the link answers errors first: the run waits for `/v1/status` to say
   degraded. Then Finder asks again for each item, as after a listing that failed. Each one is
   answered again with its badge, and the observed folder is still read each second.
3. Mounted again: Finder is told the folders without the mount, then with it, and each badge
   again. It's told only once.
4. The kernel's table is read here: `/` is in it.

The rig ([rig-up.sh](raw/setup/rig-up.sh)) was a `voidfs-server` filesystem store on
`127.0.0.1:9373` (admin `9374`), behind the live run's [relay](../item8-live/raw/live/setup/relay.py)
on `9375`. A test daemon ran with its own state under `/private/tmp/vbl.YkeiVM`, and the drive was
`badgeloss-1792199095`. Nothing was mounted. The FSKit module, the agent and the Finder extension
weren't registered or enabled. [selftest.sh](raw/setup/selftest.sh) answered the run's `ready:`
lines by signalling the relay (503, then forwarding) or restarting the daemon.

| Run | Checks | Failures |
| --- | --- | --- |
| `remount --degraded`, three runs ([1](raw/selftest/remount-degraded-1.txt), [2](raw/selftest/remount-degraded-2.txt), [3](raw/selftest/remount-degraded-3.txt)) | 21 each | 0 |
| `remount` ([run](raw/selftest/remount.txt)) | 20 | 0 |
| `main` with `--peer` and `--metrics` ([run](raw/selftest/main.txt)) | 35 | 0 |
| `daemon-restart` with `--peer` ([run](raw/selftest/daemon-restart.txt)) | 39 | 0 |
| `refused` ([run](raw/selftest/refused.txt)) | 3 | 0 |
| **Break:** the old `publish`, three runs ([1](raw/break/break-old-publish-1.txt), [2](raw/break/break-old-publish-2.txt), [3](raw/break/break-old-publish-3.txt)) | 20 each | 3 each |

- **The main run.** 200 files' badges cost no server request in 10 s, with 33 local calls for
  3 folders, all answered unchanged, as before.
- **The break.** It put back `main`'s `publish`, which compares mountpoints only, character for
  character ([old-publish.py](raw/break/old-publish.py)). In each run it failed "Finder is told
  the folders without the mount, then with it", "and the badges again", and "only once" (no call
  at all).
- **A race in the test's first draft.** It compared tell counts right after asking the model,
  before the model's queue had answered, and one check failed that way. A later request's answer
  could also count for an earlier phase. `expectBadges` now waits on the model's queue before it
  compares anything. With the old `publish` it then failed the re-tell check too. The runs above
  are of the final code, built once for them and once for the break.

## Not shown

- **Finder itself.** Nothing here ran Finder, FSKit or the registered extension. Two things need a
  live run, with the user's approval for the registration changes:
  - that a quick remount leaves a new window without badges;
  - that the model's re-telling brings them back.
- **What Finder does with an unchanged `directoryURLs`.** The fix tells Finder the folders without
  the mount first, as the slow restart in phase I did, rather than relying on the same set
  twice.

## Replay

With the app built (`xcodebuild … -derivedDataPath <dir>`) and
`cargo build -p voidfs-server -p voidfs-cli -p voidfs-sdk --example mount_peer --bins`:

```bash
RUN=$(bash raw/setup/rig-up.sh 9373 9374 9375 badgeloss-$(date +%s) <dir>/Build/Products/Release/voidfs.app)
bash raw/setup/selftest.sh "$RUN" remount-degraded remount --degraded
bash raw/setup/rig-down.sh "$RUN"
```

Building registers the build folder's copy with LaunchServices. Afterwards, `lsregister -u` the
build and delete its products ([registrations](raw/registrations.md)).
