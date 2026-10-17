# Item 8 on a live FSKit mount, 16 October 2026

Step 5, item 8. The earlier slices checked the menu against a test daemon
([menu-bar transfers](../menu-transfers/README.md)) and Finder's badges in a live Finder
([Finder Sync badges](../finder-sync-badges-live/README.md)), each without the other. This run
mounted a new drive through FSKit and looked at Finder, the menu's view and the CLI together:
- through each state;
- through the queue's controls;
- through restarts of the daemon and the agent;
- through an offline, a hung and a degraded link.

It also measured a 10,000-file import against the menu, and bounded what the menu reads. The user
approved the registration changes and their restoration first. macOS 27.0.1 (26A434),
Xcode 27.0 (27A5194q), Apple M5 Pro, branch `mount-cancel-kept` at `757a5cd` with this change's
code ([versions](raw/live/versions.txt)).

## Setup

- **Isolated rig** ([rig-up.sh](raw/live/setup/rig-up.sh)):
  - a `voidfs-server` filesystem store on `127.0.0.1:9363` (admin `9364`);
  - a TCP relay in front of it on `9365` ([relay.py](raw/live/setup/relay.py)), which makes the bad links:
    - killed: refused (offline);
    - `SIGSTOP`: hung (connections open, nothing answers);
    - `SIGUSR1`: every request answered `503 SlowDown` (degraded);
  - a test daemon with its own state under `/private/tmp/vi8.wIz8YU`, reaching the server through
    the relay;
  - a new drive, `live-1792182043`.

  The keys are the local test server's documented test pair (see `docs/spikes/fskit.md`).
  `~/Library/Application Support/voidfs` was absent throughout.
- **Selecting the dev build** ([register.txt](raw/live/setup/register.txt)):
  - `VoidfsStateDirectory` set to the test state;
  - the `/Applications` agent unregistered and this build's agent registered;
  - `lsregister -u /Applications/voidfs.app` and `pluginkit -a` on this build's module;
  - `pluginkit -e use -i dev.voidfs.app.findersync`.

  FSKit still picked older builds' modules, which can't read the `kept` state. With the user's
  approval, the three other copies were unregistered for the run
  ([register-others.txt](raw/live/setup/register-others.txt)):
  - another worktree's build;
  - two builds in another session's scratchpad.
- **Mount.** `void mount live-1792182043 <run>/mnt`, adapter `fskit`
  ([mount.txt](raw/live/setup/mount.txt)).
- **Capture.** At each moment, [capture.sh](raw/live/setup/capture.sh) recorded:
  - `void status`;
  - `void uploads --all`;
  - the menu's lines (`voidfs transfers`);
  - the daemon's `POST /v1/mounts/states` for three folders, which is what Finder's badges come from;
  - with a label ending in `menu`, the menu's view drawn offscreen (`transfers-selftest --look --png`).

  [link.sh](raw/live/setup/link.sh) timed each command during the link phases. Computer use
  screenshotted Finder windows on the mount.
- **Uploads limited to 2 KiB/s** from phase B on, so that a change stays in flight long enough to
  see.

## Results

Finder, the menu and the CLI agreed at every capture. The table's file names link the Finder
screenshots; "menu" links the menu's drawing.

| Phase | What was done | CLI and menu | Finder |
| --- | --- | --- | --- |
| A, saved | `live/`, `saved.txt`, `conflict.txt`, `nested/inner.txt` written and uploaded | `uploads idle` in both ([status](raw/live/a-status.txt), [menu](raw/live/a-transfers.txt)) | green checks ([a](raw/live/a-finder.jpg)) |
| B, saving | 2 KiB/s limit; `kept.bin` (40 KiB), `big.bin` (160 KiB) written; an 8-file import; `mount_peer` put `conflict.txt` | "8 uploading · 217.6 KiB to send · 1.5 KiB/s · limit 2.0 KiB/s" in both; menu lists the mount's changes and `import8 25%` ([menu](raw/live/b-menu.png)) | blue arrows on `kept.bin`, `big.bin` ([b](raw/live/b-finder.jpg)) |
| C, kept | menu model: limit, pause/resume all, drive, change; cancel change 8 (`kept.bin`) mid-flight ([c-cancel](raw/live/c-cancel.txt), 16 checks) | "1 file kept on this Mac, not uploaded" in both; menu says what the cancel did ([menu](raw/live/c-menu-cancel.png)) | `kept.bin` purple "not uploaded" ([c](raw/live/c-finder.jpg)) |
| D, paused | `void uploads pause --drive`; `conflict.txt` edited; `mount_peer` put it again; `waiting.bin` | "4 queued · 4 paused · paused on …" in both ([menu](raw/live/d-menu.png)) | `conflict.txt`, `waiting.bin` saving ([d](raw/live/d-finder.jpg)) |
| G, conflict | `void uploads resume --drive` ([g-resume](raw/live/g-resume.txt)) | "2 queued, 1 in conflict"; change 24 "conflict: the drive changed (now version 29.0); this Mac keeps its copy" ([menu](raw/live/g-menu.png)) | orange triangle on `conflict.txt`; `live` folder conflict at the drive root ([g](raw/live/g-finder.jpg), [root](raw/live/g-finder-root.jpg)) |

**Restarts.**
- **Daemon stopped (H).** `void daemon stop` also ended the FSKit mount
  ([h-daemon-stopped](raw/live/h-daemon-stopped.txt)). `void status`, `voidfs transfers` and the
  menu's view said the same thing from the journal: "3 changes (8 B) wait in the journal, for the
  daemon to start · 1 file kept on this Mac, not uploaded" ([menu](raw/live/h-menu-no-daemon.png)).
  Before this change, the menu showed only that no daemon ran.
- **Daemon started (I).** The mount came back about 2 s after `void daemon start`
  ([i-daemon-started](raw/live/i-daemon-started.txt)). The queue, the kept file and the conflict
  came back in the CLI, the menu ([i-transfers](raw/live/i-transfers.txt)) and Finder
  ([i](raw/live/i-finder-after-daemon-restart.jpg)).
- **Agent killed (J).** The agent is the bridge between the FSKit module, Finder's extension and
  the daemon. After `kill`, launchd relaunched it in about 2 s (runs 2, new pid,
  [j-agent-restart](raw/live/j-agent-restart.txt)). The mount listed and wrote through it.
  `after-agent.txt` was badged saved ([j](raw/live/j-finder-after-agent-restart.jpg)).
- **The app.** The menu app holds no state. Every `voidfs transfers` and `transfers-selftest` above
  is a new app process, and each showed the daemon's state. The running menu app wasn't quit and
  reopened by hand.

**Links.** Each phase wrote a file on the mount, then timed the CLI, the menu's text, the menu's
controls with its drawing, the queue, and the badge call ([link.sh](raw/live/setup/link.sh)).

| Phase | Link | Status says | Timings | Notes |
| --- | --- | --- | --- | --- |
| K | relay killed: refused | "offline: uploads wait, and reads of what isn't cached fail" in both | write 0.06 s, status 0.02 s, menu's self-test, 12 checks, 1.25 s | `offline.txt` badged saving ([k](raw/live/k-offline-finder.jpg), [menu](raw/live/k-offline-menu.png)) |
| L | relay back | online | | `offline.txt` done 15 s later ([l](raw/live/l-back-online.txt)) |
| M | relay stopped: hung | **online** | write 0.05 s, status 0.02 s; **the drive pause hung**: the self-test's alarm fired at 60 s, and `void uploads pause --drive` failed after 30 s ([m-hung](raw/live/m-hung.txt), [m-hung-pause](raw/live/m-hung-pause.txt)) | an entry's pause answered at once; `hung.txt` stayed "uploading" for 5 minutes and finished 1 s after the relay resumed ([n](raw/live/n-hang-resumed.txt)) |
| P | hung, daemon with the fix | online | menu's self-test, 12 checks, 0.68 s, drive pause and resume included ([p](raw/live/p-hung-fixed.txt)) | `hung2.txt` done 1 s after the relay resumed |
| Q | every request 503 | "degraded: errors or slow answers" in both | controls 0.66 s; status 0.02 s | writing `slow.txt` and listing `live/` failed with EIO ([q](raw/live/q-degraded.txt), [menu](raw/live/q-degraded-menu.png)) |
| R | relay forwards again | online at 16:49:24 | | listing and `slow.txt` worked at once; the status said degraded for 79 s ([r](raw/live/r-recovered.txt)) |

- **The hang (M) is fixed in this change.** A drive's pause, resume or cancel asked the server
  for the drive's description (`Sessions::upload_names` → `describe_drive`), which waits out a hung
  link: 120 s per attempt. The fix, `Sessions::control_names`, takes the drive's spellings from the
  drive open on this Mac or from the alias memo. Only a name this Mac never saw asks the server, and
  the queue holds only names this Mac resolved. A daemon test hangs a proxy mid-run and checks that
  pause by alias, resume by id and cancel all answer within 3 s
  (`drive_controls_answer_while_the_link_hangs`). That test fails without the fix.
- **Hung looks online.** No request had failed: the only one in flight was `hung.txt`'s upload,
  whose attempt allows 120 s. The status said online for the 5 minutes the relay was stopped.
  Detecting a hung server sooner is a follow-up.
- **Degraded mount reads.** The daemon had restarted 44 s before phase Q. Its cache held nothing for
  `live/`, so reads of it needed the server and failed with EIO, by design while the server answers
  errors. The menu's and the CLI's controls answered.
- **Degraded lasts past recovery.** The status turns online at the first answer after the 30 s
  window. With nothing to upload, that answer was the feed's watch reconnecting, which by then was
  retrying up to 30 s apart (`feed.rs` `backoff`).
- **Degraded during a bandwidth limit.** Phase C's status also said "degraded: errors or slow
  answers", with the relay forwarding ([c-status](raw/live/c-status.txt)). Uploads sent at 2 KiB/s
  run past the 10 s that `Connectivity` counts as a slow answer (`connectivity.rs`,
  `o.elapsed > cfg.slow`). Three of them in 30 s made the link degraded. A limit the user set
  shouldn't read as a degraded server: a follow-up.

**The drive's own icon** ([c-parent-stale](raw/live/c-parent-stale.jpg),
[d](raw/live/d-parent-reopened-stale.jpg), [e](raw/live/e-parent-no-badge.jpg),
[f window](raw/live/f-parent-no-badge-after-request.jpg),
[f desktop](raw/live/f-desktop-volume-no-badge.jpg), [log](raw/live/f-diag-stream.txt)):
- The mounted volume's icon in its parent folder kept a "saving" badge after the drive's state
  became kept. It still showed after the window was reopened.
- After a remount, it had none.
- A diagnostic build briefly registered the parent folder too (now reverted). Finder asked for
  badges on every other item in the parent folder, never for the volume's icon. A badge set on the
  volume's URL unasked (`setBadgeIdentifier`) didn't show.
- Finder asked once for the volume's root during a remount. The extension answered `kept`, and
  neither the window nor the desktop icon showed it.

So the extension can't keep the drive icon current. The drive's state shows on the folders inside
it, at the drive root ([g root](raw/live/g-finder-root.jpg)), and in the menu.

**Other findings.**
- **Rebuilding unmounts.** Rebuilding the app bundle re-registers it, which ends the FSKit module.
  The volume unmounted, and the daemon still said "mounted" until `void unmount` and `void mount`
  ([rebuild-unmounted](raw/live/rebuild-unmounted.txt)). Later rebuilds unmounted first
  ([e-unmount](raw/live/e-unmount-for-rebuild.txt), [e-remount](raw/live/e-remount.txt)).
- **SENT stays at 0%.** A mount's uploads show 0% until they finish (phases B and C).
- **Cancel on queued edits.** The menu shows Cancel on a mount's change queued behind a conflict
  (changes 25 and 26). Cancel there changes nothing, and the menu says "Nothing was cancelled: …
  it waits for a conflict". The button could be hidden.
- **Drive ids in mount rows.** The menu's and `void uploads`' rows show a mount's changes by drive
  id, and imports by the name used.
- **No badges after the 503 phase.** A Finder screenshot of `live/` at 16:51, about 2 minutes after
  the status said online, showed no badges at all ([r](raw/live/r-recovered-finder.jpg)). The
  earlier phases, the daemon restart included, showed every badge. Not investigated before the
  rig came down.

## The 10,000-file import

A separate test rig ([raw/large-import](raw/large-import/)) had:
- 10,000 one-KiB files in 20 folders, queued with `void upload --detach` and paused;
- about 10,000 more finished entries in the journal.

[timings.txt](raw/large-import/timings.txt) has the numbers. Before this change:
- every `/v1/status` read the whole journal;
- the watch the menu follows sent every item each second, about 1.4 MB a line.

| Read | Before | After |
| --- | --- | --- |
| `/v1/status` | median 79.4 ms | median 16.6 ms |
| `/v1/uploads` | 120.1 ms | 59.1 ms |
| `/v1/uploads?limit=6` | n/a | 16.8 ms, 1,622 bytes |
| daemon CPU, one watch | 11.9% (unlimited) | 6.5% unlimited, 1.9% limited |
| daemon CPU, the menu app's watch and status | n/a | 2.8% |
| menu app | 1.4% CPU, 32 MB footprint | 0.1% paused, 0% uploading; 16 MB |

These times are from a debug daemon build. While uploading at 64 KiB/s (about 64 one-KiB files a
second), the daemon used 55–58% CPU with or without the change. That cost is the uploads, not the
menu. The changes:
- `Queue::unfinished` answers status from memory;
- `?limit=` lists the rows the menu shows, those that most need showing first, and counts the rest
  (`more`, `mountUnsent`);
- the menu asks for 6.

## Restoration

[restore.txt](raw/live/setup/restore.txt) logs each step:
- the test Finder window closed, and only it (the user's own window stayed open);
- `void unmount`; the daemon, server and relay stopped ([rig-down.sh](raw/live/setup/rig-down.sh));
- `pluginkit -e default` for the Finder extension, and `pluginkit -r` on this build's two appexes;
- this build's agent unregistered, `lsregister -u` on this build;
- `lsregister -f -R /Applications/voidfs.app`, `pluginkit -a` on its module, and its agent
  registered;
- `VoidfsStateDirectory` deleted;
- the three other copies registered again (`lsregister -f -R`, `pluginkit -a` on their modules and
  on the one Finder extension that was registered before).

[registrations-after.txt](raw/live/registrations/registrations-after.txt) differs from
[registrations-before.txt](raw/live/registrations/registrations-before.txt) only in:
- the registrations' UUIDs;
- the times they were made;
- LaunchServices' internal ids.

Everything else matches:
- the same paths and Finder Sync elections (Space's enabled, voidfs's at default);
- the agent loaded and not running, as before;
- no voidfs defaults and no mounts;
- `~/Library/Application Support/voidfs` absent.

## Not shown

- **Clicks in the real menu-bar menu.** The app's menu was drawn offscreen and driven through its
  model. Computer use can't reach a development app run from a build folder.
- **Finder's context menu on this run.** Shown in
  [the earlier live run](../finder-sync-badges-live/README.md).
- **The running menu app restarted by hand.** Each check is a new app process.
- **Two Macs.**
- **Finder screenshots for phases P and Q.** Screen capture failed then ("audio/video capture
  failure") and worked again for R.
- **The missing badges after phase Q** (above), not investigated.
- **`void unmount <drive name>`.** It resolves the name through the server too
  (`crates/voidfs-daemon/src/mounts.rs`), so it can wait on a hung link like the drive pause did.
  Not tried; a follow-up.

## Replay

On a Mac with the app built (`xcodebuild … -derivedDataPath apps/macos/build`) and
`cargo build -p voidfs-server -p voidfs-cli --examples`, and with the user's approval for the
registration changes:

```bash
bash raw/live/setup/rig-up.sh 9363 9364 live-$(date +%s) 9365
```

`rig-up.sh` names this worktree in `REPO`; change it to your checkout first.

Then:
1. Source the printed run's `env.sh` and run [register.sh](raw/live/setup/register.sh) `<run>`.
2. `void mount "$RIG_DRIVE" "$RIG_RUN/mnt"`.
3. Make each phase's files as in the `*-make.txt` logs.
4. Run `capture.sh <label> [png]` and `link.sh <label> <file>`.
5. Signal the relay to change the link: `kill -STOP`, `-CONT`, `-USR1`, `-USR2` on
   `setup/relay.pid`; killing it refuses connections.
6. Restore as above, then compare with [snapshot.sh](raw/live/setup/snapshot.sh).

For the import, queue 10,000 small files with `void upload --detach` and `void uploads limit 64KiB`.
Time `curl --unix-socket … /v1/status` and `/v1/uploads?limit=6`, and run
[sample.sh](raw/large-import/sample.sh) on the daemon and the app.
