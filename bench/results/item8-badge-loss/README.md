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

## The live check

After the Mac was restarted, FSKit reported the module switched on (`enabled=true` for
`/Applications`), though the user found System Settings' switch still wouldn't change. The check
ran on 16 October from 21:59 to 22:12, with the user's approval of the registration changes. On
a live FSKit mount, a quick remount left a new Finder window without badges with the old
comparison, and the fix brought them back ([evidence](raw/live/rerun/)).

- **The rig.** As above, a `voidfs-server` filesystem store behind the relay (`127.0.0.1:9385`),
  a test daemon with its own state under `/private/tmp/vbl.CpdeYu`, and the drive
  `remount-1792202425`, here mounted through FSKit at the run's `mnt`. `live/` held four saved
  files and `held.txt`, a change held by the drive's pause, so it stayed saving
  ([mount-and-fill](raw/live/rerun/mount-and-fill.txt)).
- **The builds** ([builds](raw/live/rerun/builds.txt)). The pre-fix build took `Badges.swift` and
  `BadgesSelfTest.swift` from `28cec07` and everything else from this branch; its extension
  doesn't import `getfsstat`. It was registered with [register.sh](raw/live/setup/register.sh),
  with the other copies in [others.txt](raw/live/setup/others.txt) unregistered
  ([register-old](raw/live/rerun/register-old.txt)). After its trials, the drive was unmounted
  and the app rebuilt in place from this branch's sources
  ([rebuild](raw/live/rerun/g-rebuild-fixed.txt)), then registered and mounted again. The Finder
  extension then ran from the rebuild ([mount](raw/live/rerun/j-mount-fixed.txt)).
- **Each trial** ([trial.sh](raw/live/setup/trial.sh)) closed the last test window, ran `void
  daemon stop; void daemon start` as one command, which unmounts the drive and mounts it again, as
  phase O did, waited for `live/` to be there, and opened it in Finder
  ([open-live.sh](raw/live/setup/open-live.sh)). Then the window was captured. The control waited
  5 s between the stop and the start, as phase I's slow restart did.

The remount and open times are fskitd's and Finder's ([log](raw/live/rerun/logs/mounts-and-opens.txt));
a capture's time is when it was saved, within about a second of the capture. Captures of the same
view at the same size are the same bytes, so several files are identical.

| Trial | Build | Volume gone | Finder opened `live` | Captures, after the open | Badges |
| --- | --- | --- | --- | --- | --- |
| a, baseline ([log](raw/live/rerun/a-baseline-old.txt)) | old | (mounted 22:00:39.9) | 22:01:02.8 | 6.1 s ([a](raw/live/rerun/a-baseline-old.jpg)) | all |
| b ([log](raw/live/rerun/b-old-trial1.txt)) | old | 0.23 s | not opened | | opened before the mount was back; not counted |
| c ([log](raw/live/rerun/c-old-trial2.txt)) | old | 0.23 s | 22:02:20.1 | 2.1 s ([c1](raw/live/rerun/c-old-trial2-first.jpg)), 36.4 s ([c2](raw/live/rerun/c-old-trial2-after-37s.jpg)) | none, none |
| d ([log](raw/live/rerun/d-old-trial3.txt)) | old | 0.23 s | 22:03:05.8 | 5.1 s ([d](raw/live/rerun/d-old-trial3-first.jpg)) | none |
| e, slow control ([log](raw/live/rerun/e-old-slow-control.txt)) | old | 5.26 s | 22:03:30.6 | 6.2 s ([e](raw/live/rerun/e-old-slow-control.jpg)) | all |
| f ([log](raw/live/rerun/f-old-trial4.txt)) | old | 0.25 s | 22:03:51.1 | 5.2 s ([f](raw/live/rerun/f-old-trial4-first.jpg)) | none |
| k, baseline ([log](raw/live/rerun/k-fixed-baseline.txt)) | fixed | (mounted 22:06:01.6) | 22:06:07.8 | 6.2 s ([k](raw/live/rerun/k-fixed-baseline.jpg)) | all |
| l ([log](raw/live/rerun/l-fixed-trial1.txt)) | fixed | 0.25 s | 22:06:23.4 | 2.2 s ([l](raw/live/rerun/l-fixed-trial1-first.jpg)) | all |
| m ([log](raw/live/rerun/m-fixed-trial2.txt)) | fixed | 0.25 s | 22:06:39.6 | 2.3 s ([m1](raw/live/rerun/m-fixed-trial2-first.jpg)), 102.4 s ([m2](raw/live/rerun/m-fixed-trial2-later.jpg)) | none, all |
| n ([log](raw/live/rerun/n-fixed-trial3.txt)) | fixed | 0.24 s | 22:09:01.1 | 1.7 s ([n1](raw/live/rerun/n-fixed-trial3-first.jpg)), 3.4 s ([n2](raw/live/rerun/n-fixed-trial3-second.jpg)) | none, all |

- **The old build reproduced phase O.** Three windows opened after a quarter-second remount had
  no badge at all, 5 s or more after Finder opened them, longer than the model's 3 s between
  reads; one still had none after 36 s. While it had none, the daemon's folder call said
  `held.txt` was saving ([states](raw/live/rerun/c-old-trial2-states.json)). The same build
  badged every item in the baseline and after the 5 s restart.
- **The fixed build badged every window.** Each of three windows opened after the same remount got
  every badge. In n, the badges weren't there 1.7 s after the open and were all there 3.4 s after
  it, 3.7 s after the mount: the model's next read of the mounts saw the new volume and told Finder
  again. m's first capture, also before that read, had none; its next had all. l's first capture
  came after the read.
- **What the check doesn't time.** Only n places the badges' return within one read; m's
  isn't placed closer than 102 s. One old window was watched past 6 s.
- **This run lost the switch once, and the user put it back.** The app was rebuilt in place while
  it was the only registered copy of the module. fskit_agent logged "Removed 1 identifiers" at
  22:04:37 ([log](raw/live/rerun/logs/fskit-agent-identifiers.txt)), and the rebuild, registered
  again, said `enabled=false` ([module](raw/live/rerun/i-reregister-module.txt)). The user
  switched it on in System Settings, which worked this time (`enabled=true` at 22:05:52). A rig
  that rebuilds should keep another copy registered, or build into a second folder.
- **Restored** ([rig-down](raw/live/rerun/z-rig-down.txt), [restore](raw/live/rerun/z-restore.txt),
  [fix-ups](raw/live/rerun/z-restore-fixups.txt)):
  - only the run's Finder windows were closed, found by window id;
  - the drive's uploads were resumed and it was unmounted, and the rig was stopped;
  - the other copies and `/Applications` were registered again before the build was
    unregistered, and no identifier was removed;
  - the agent was registered from `/Applications` again, `VoidfsStateDirectory` was deleted, and
    the Finder Sync election set back to use;
  - the build folder was deleted.

  The restore also registered `compassionate-payne`'s Finder extension, which hadn't been
  registered before, and pluginkit then listed it for Finder Sync. It was removed again and
  `sleepy-germain`'s added back. [registrations-after](raw/live/rerun/registrations-after.txt)
  equals [registrations-before](raw/live/rerun/registrations-before.txt), apart from ids, times
  and order, with two differences: pluginkit shows the module's version as `(null)` rather than
  `0.1`, and FSKit names `sleepy-germain`'s copy rather than `/Applications`'s. Both copies are
  registered as before, and FSKit still says `enabled=true`.

### The first attempt, blocked

The user approved a live run on 16 October. The first attempt ran from 21:31 to 21:50 and stopped
before the first mount, because FSKit had the voidfs module switched off.

- **The plan.** Build the app from `28cec07`'s Swift sources, which have the old comparison, and
  register it ([register.sh](raw/live/setup/register.sh); the other copies in
  [others.txt](raw/live/setup/others.txt) unregistered). Then repeat phase O's quick remount
  and screenshot the new window. Then rebuild
  with the fix in the same place and repeat.
- **The mount was refused** ([mount-refused](raw/live/mount-refused.txt)): `Module
  dev.voidfs.app.fskit is disabled!`. `voidfs status` said `enabled=false` for this build
  ([register-scratchpad](raw/live/register-scratchpad.txt)). The `/Applications` copy, registered
  again later in the run to check, said the same ([register-applications](raw/live/register-applications.txt)).
- **The switch was already off.**
  - In the live run, at about 16:20, FSKit reported another build's copy enabled
    ([register.txt](../item8-live/raw/live/setup/register.txt)).
  - At 16:52:41, that run's restore unregistered its build before registering `/Applications`
    again. fskit_agent logged "Removed 1 identifiers", then "Added 1 identifiers"
    ([log](raw/live/logs/fskit-agent-identifiers.txt)).
  - This run didn't remove the identifier before its first check (its first build was registered
    at 21:33, before the other copies were unregistered), and the check already said
    `enabled=false`.

  So FSKit seems to forget the switch when every copy of a module is unregistered at once. That is
  an inference from these logs: FSKit keeps the switch in a protected container. The second
  attempt saw it happen: rebuilding the only registered copy removed the identifier, and the
  switch came back off.
- **System Settings wouldn't switch it on.** The user tried for this build in the scratchpad and
  again for a rebuild in the worktree ([register-worktree](raw/live/register-worktree.txt)).
  Every attempt failed in Login Items with `Failed to enabled FSExtension:
  NSPOSIXErrorDomain Code=1` (EPERM), before any call reached fskitd ([log](raw/live/logs/switch-refused.txt)).
  LaunchServices recorded the user's election ([elections](raw/live/logs/elections.txt)), but that
  isn't FSKit's switch. `pluginkit -e use` changes only that election too
  ([module-pluginkit](raw/live/module-pluginkit.txt)), and it was put back to default (the
  restore script set it then; it now takes the election to put back).
- **Restored** ([rig-down](raw/live/rig-down.txt), [restore.sh](raw/live/setup/restore.sh),
  [restore](raw/live/restore.txt)):
  - nothing was mounted, and no Finder window was opened;
  - this run's agent was unregistered and `/Applications`'s registered again;
  - the build was unregistered and both its folders were deleted;
  - the other copies were registered again, and `VoidfsStateDirectory` deleted.

  [registrations-after](raw/live/registrations-after.txt) equals
  [registrations-before](raw/live/registrations-before.txt), apart from ids, times and order.
  FSKit's switch was still off, as it was before the run, until the Mac was restarted.

## Not shown

- **How fast the badges come back, in general.** The live check places one return within a read
  of the mounts (3.7 s after the mount). A read can be up to 3 s after a remount.
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

The live check, with the rig up and the app built: `register.sh <run> <app>`, mount the drive and
fill `live/` ([mount-and-fill](raw/live/rerun/mount-and-fill.txt)), then, with the rig's `env.sh`
sourced, `trial.sh <label>` for each trial and `restore.sh <run> <app> <election>`. Keep another
copy of the module registered while rebuilding, or FSKit switches the module off.
