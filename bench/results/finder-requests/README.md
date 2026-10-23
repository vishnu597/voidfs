# Finder's requests on big folders, 22 October 2026

*Step 5, [item 9](../../../docs/step-5-macos.md#item-9-protocol-additions-only-where-measured):
"1,000/10,000-file listings and xattr-heavy Finder folders have recorded request counts and
timings". One Mac (Apple M5 Pro, macOS 27.0.1, 26A434). The user approved this live run on
22 October.*

## What this shows

- **Finder asks about the files it shows, not about the folder.** A Finder window of 1,000 or of
  10,000 files cost one listing page per 1,000 names, plus attrs for the 31 files a 1000 × 700
  point list window shows (42 in icon view, which also reads those 42 files' bytes for their
  thumbnails). Reopening the window read the next 44 rows in list view and 7 more icons in icon
  view, the same for 1,000 and 10,000 files. A plain folder, whose listing says no file has
  xattrs, cost no attrs requests at all.
- **A second look costs nothing.** On the same mount, a folder opened again made no listing
  request; after a remount with the daemon's state kept, it made no attrs requests (the maps are
  remembered), only the listing pages again.
- **Whole-folder walkers pay one request per file with xattrs, one at a time.** `ls -l@` made 1,000
  attrs requests for 1,000 such files and took 8.8 s on loopback and 25.1 s with each request
  held 12 ms; for 10,000 files, 84.6 s and 245.4 s. getattrlistbulk, Finder's listing call, took
  0.08–0.6 s for the same folders. **Raycast's file indexer** walked the whole drive, 11,000 attrs
  requests, within 50 s of every cold mount under `~/voidfs` ([below](#raycast)).
- **Two costs were voidfs's, and are fixed in `5e8a907`.** Readers of one file didn't share a fetch,
  so Finder's first look made up to 68 attrs requests for 42 files at 12 ms; it now makes one a file.
  And each fetched map was saved with a full sync, about 5 ms a file: `ls -l@` over 1,000 files went
  from 9.8 s to 4.7–5.0 s on loopback and from 25.6 s to 21.6 s at 12 ms ([the fix](#the-fix)).

So local memoization meets item 9's need for Finder windows. It doesn't for walkers: they still cost
one attrs request per file with xattrs, which only xattrs in the listing (`?x-voidfs-list`, an RFC)
would remove. That decision is the user's.

## The rig

- The installed `/Applications/voidfs.app` (`45c449f`, notarized on 20 October): its FSKit
  module and bridge agent, and its `void` as the daemon for the baseline runs. No app was built,
  installed or registered.
- A throwaway `voidfs-server` (release, `main` at `1910880`; no server change since 16 October) on
  a file store at 127.0.0.1:9381, with a key made for the run.
- [`request-relay.py`](../../../apps/macos/scripts/request-relay.py) at 127.0.0.1:9380 in front of
  it: the daemon's only way to the server. It logs every request (method, path, query, status,
  time) and can hold each one a set time before forwarding it. Asked for 12 ms, it held requests
  13.8 ms (p50, [below](#the-fix)); "12 ms" below means that.
- A private daemon whose state directory the installed agent was pointed at
  (`VoidfsStateDirectory` and an agent restart), mounting the drive at a hidden path,
  `~/.voidfs-finder-rig/mnt`.
- The drive `finder`, filled by
  [`finder_seed`](../../../crates/voidfs-sdk/examples/finder_seed.rs) straight to the server:
  under each tag, `plain-1k/`, `xattrs-1k/`, `plain-10k/` and `xattrs-10k/` of small text files.
  Every file in an `xattrs-` folder has `com.apple.provenance` (11 bytes); every third also has
  `com.apple.quarantine` and `kMDItemWhereFroms`, every tenth a colour label in
  `com.apple.FinderInfo` and a user tag. [The seed's output](raw/output/seed.txt).

[`setup.sh`](raw/rig/setup.sh) does all of this; [`restore.sh`](raw/rig/restore.sh) undid it.
[`cold.sh`](raw/rig/cold.sh) empties the daemon's cache and namespace and mounts again;
[`remount.sh`](raw/rig/remount.sh) remounts with the daemon's state kept. Both take `VOID`, the
daemon's binary.

## How it was measured

[`finder-requests.py`](../../../apps/macos/scripts/finder-requests.py) runs each step and reads the
relay's log. A step starts just before its action and ends when no request has finished for 10 s.
Its requests are split into the folder's own (its listing pages and requests naming its files) and
the rest, counted by folder. The span runs from the action's start to the end of the folder's last
request; for the shell, the command's own time is given too.

Finder steps, for each folder: `cold` (state emptied, then a new 1000 × 700 point window in list or
icon view, pointed at the folder), `reopen` (that window closed, a new one opened), `remount` (closed,
the drive remounted with the daemon's state kept, a new window). Shell steps, each cold: `ls -l@`
(every file's xattr names and sizes) and getattrlistbulk over the folder
([`bulkstat.c`](../../../apps/macos/scripts/bulkstat.c)), which reads no xattrs.

Three things about Finder shaped the method; the first two runs, which didn't allow for them, are
superseded ([`l0-first-method.jsonl`](raw/results/l0-first-method.jsonl)):
- **Finder reads the visible files late, and later in a bigger folder:** 0.2–0.6 s after the
  listing for 1,000 files, 2.3–5.8 s for 10,000. With 3 s of quiet a step could end before them,
  and the next step got them (75 attrs requests on a reopen that makes 44).
- **Finder remembers a folder it has shown**, its scroll position among other things: a folder
  whose last file had been selected came back cold with both its first and its last rows read.
  So every Finder run used a tag whose folders this Finder had never shown (`l0b`, `i0`, `l12`,
  `i12`, `fl12`, `fi12`), and the windows were never scrolled. AppleScript's
  `select last item of …` was tried as a scroll and dropped: it makes Finder read every item
  of the folder, at about 6 a second.
- **After a mount, Finder lists again the folders it showed earlier** in the run (never more than
  two), with no window open on them, and after a cold mount reads their visible files' xattrs again
  (35–59 attrs requests). Those are the "elsewhere" counts, kept out of each folder's own.

A window that shows a volume when it unmounts stays open, showing the Computer, so the runner
closes its windows by their ids. Raw output is in [`raw/output/`](raw/output), one JSON record a
step in [`raw/results/`](raw/results), and every request in
[`requests.jsonl.gz`](raw/requests.jsonl.gz).

## Finder

Each folder's own requests, on the installed daemon (`45c449f`). attrs counts each request; "files"
counts the files they were for.

| View, delay | Folder | Cold: listing pages + attrs (files) + GETs | Cold span | Reopen | Remount |
|---|---|---|--:|---|---|
| List, 0 ms | `plain-1k` | 1 + 0 + 0 | 0.30 s | nothing | nothing |
| | `xattrs-1k` | 1 + 32 (31) + 0 | 1.02 s | 44 attrs (44) | nothing |
| | `plain-10k` | 10 + 0 + 0 | 0.36 s | nothing | nothing |
| | `xattrs-10k` | 10 + 31 (31) + 0 | 2.88 s | 44 attrs (44) | 10 listing pages |
| List, 12 ms | `plain-1k` | 1 + 0 + 0 | 0.39 s | nothing | nothing |
| | `xattrs-1k` | 1 + 40 (34) + 0 | 1.23 s | 41 attrs (41) | nothing |
| | `plain-10k` | 10 + 0 + 0 | 0.50 s | nothing | nothing |
| | `xattrs-10k` | 10 + 34 (31) + 0 | 6.53 s | 44 attrs (44) | nothing |
| Icon, 0 ms | `plain-1k` | 1 + 0 + 42 | 1.12 s | 7 GETs | nothing |
| | `xattrs-1k` | 1 + 45 (42) + 42 | 1.18 s | 7 attrs (7), 7 GETs | 1 listing page |
| | `plain-10k` | 10 + 0 + 42 | 3.02 s | 7 GETs | nothing |
| | `xattrs-10k` | 10 + 46 (42) + 42 | 6.26 s | 7 attrs (7), 7 GETs | nothing |
| Icon, 12 ms | `plain-1k` | 1 + 0 + 42 | 1.31 s | 7 GETs | nothing |
| | `xattrs-1k` | 1 + 68 (42) + 42 | 1.48 s | 7 attrs (7), 7 GETs | nothing |
| | `plain-10k` | 10 + 0 + 42 | 6.44 s | 7 GETs | nothing |
| | `xattrs-10k` | 10 + 66 (42) + 42 | 6.91 s | 7 attrs (7), 7 GETs | nothing |

The remount column is the window's own step. The remount itself, before the window, brought each
folder's listing pages again in most runs (1 or 10), and never an attrs request
([results](raw/results)). In list view, cold read rows f0000–f0030 and reopen f0031–f0074: Finder
read the next screenful once the folder was cached. The cold spans are mostly Finder's own pacing:
the listing pages came within 0.15 s of the first, and Finder asked for the visible files' xattrs
and thumbnails 0.2–0.6 s later for 1,000 files and 2.3–5.8 s later for 10,000.

## The shell

| Folder | `ls -l@`, 0 ms | `ls -l@`, 12 ms | getattrlistbulk, 0 ms | getattrlistbulk, 12 ms |
|---|---|---|---|---|
| `plain-1k` | 1 listing page, 0.30 s | 1, 0.37 s | 1, 0.23 s | 1, 0.18 s |
| `xattrs-1k` | 1 + 1,000 attrs, 8.76 s | 1 + 1,000, 25.13 s | 1, 0.08 s | 1, 0.12 s |
| `plain-10k` | 10, 2.59 s | 10, 2.79 s | 10, 0.44 s | 10, 0.56 s |
| `xattrs-10k` | 10 + 10,000 attrs, 84.66 s | 10 + 10,000, 245.44 s | 10, 0.42 s | 10, 0.60 s |

Times are the commands' own. Every attrs request was for a different file, one at a time.

## Raycast

Mounted cold at the default `~/voidfs/finder`, with no window on it, the drive (then only the
untagged four folders) got 11,027 requests in 49 s: 3 drive lookups, 24 listing pages and 11,000
attrs requests, one for every file with xattrs, never more than 2 at once
([log](raw/raycast-crawl-cold.jsonl.gz)). Polling `lsof` on the mount caught pid 1720, "Raycast Beta
Backend", holding `plain-10k` and `xattrs-10k` open ([lsof](raw/raycast-lsof.txt)). It happened
again on the next cold mount. Mounted at the hidden rig path instead, the mount got its own 5
requests and nothing else in 30 s. Spotlight doesn't index the volume (step 5, 12 October).
Whether Raycast would walk a `/Volumes` mount wasn't tried.

## The fix

`5e8a907` changes two things in the mount core's xattr snapshot
([`mount_xattrs.rs`](../../../crates/voidfs-client/src/mount_xattrs.rs)):
- Readers of one inode (getxattr and listxattr) wait for one another's remote fetch and find its
  map. A change still takes its own snapshot and never waits behind a read, so a local change
  still wins an in-flight fetch.
- A clean map is saved on the store's NORMAL connection, as staged writes are, rather than with
  a full sync: it is a cache, fetched again if a crash loses it. The journal and dirty maps keep
  the full sync, and a later full commit makes the earlier ones durable too.

[`concurrent_readers_share_one_remote_snapshot`](../../../crates/voidfs-client/tests/mount_xattrs.rs)
holds one fetch and starts two more readers: 1 request with the fix, 3 without it
([break](raw/break.txt)). All 16 xattr tests and the rest of `voidfs-client`'s pass.

**`ls -l@` over `s1/xattrs-1k`, old and new daemon**, each cold, alternating builds (A is the
installed `45c449f`, B is built here from `d064624`, the same code as `5e8a907` before a comment
and its message were corrected; the daemon log names the build of every start,
[log](raw/output/daemon.log)):

| Delay | Runs, in order | `ls -l@` | Time on this Mac between one response and the next request (p50) |
|---|---|---|---|
| 0 ms | A, B, B, A | 9.83, 5.02, 4.72, 9.89 s | 8.88, 4.13, 3.91, 8.89 ms |
| 12 ms | B, A, A, B | 21.56, 25.63, 25.64, 21.75 s | 7.30, 11.27, 11.22, 7.44 ms |

Each run made the same 1,001 requests. The rest of a file's time on this Mac, about 4 ms after the
fix, wasn't broken down: it covers the kernel's calls through FSKit, the bridge and the daemon, and
the relay itself.

**Finder at 12 ms with the new daemon**, on folders it had never shown (`fl12`, `fi12`):

| View | Folder | Cold: listing pages + attrs (files) + GETs | Before (`l12`, `i12`) | Reopen | Remount |
|---|---|---|---|---|---|
| List | `xattrs-1k` | 1 + 31 (31) + 0 | 1 + 40 (34) | 44 attrs (44) | nothing |
| | `xattrs-10k` | 10 + 31 (31) + 0 | 10 + 34 (31) | 44 attrs (44) | nothing |
| Icon | `xattrs-1k` | 1 + 42 (42) + 42 | 1 + 68 (42) | 7 attrs (7), 7 GETs | nothing |
| | `xattrs-10k` | 10 + 42 (42) + 42 | 10 + 66 (42) | 7 attrs (7), 7 GETs | nothing |

## Limits

- One Mac, a loopback server and an emulated round trip. The 12 ms is a fixed hold per request in
  a Python relay, not a network; the relay adds its own time on loopback too.
- One window size, list and icon view only; no column view, Quick Look, Get Info, copying out or
  Spotlight. Finder runs were single runs each; only the `ls -l@` comparison alternated builds.
- The installed module and agent were used throughout; only the daemon changed for the fix. No
  logout, reboot or second Mac.
- The Raycast crawl is one app's behaviour on this Mac, seen twice; other indexers and backup
  tools weren't tried.

## The rig afterwards

[`before.txt`](raw/before.txt) and [`after.txt`](raw/after.txt): the same macOS, the installed
`45c449f`, the agent running, no `VoidfsStateDirectory`, no mount, no daemon on the default socket,
no default state directory. The relay, server and private daemon were stopped, every window the
runs opened was closed, and `~/.voidfs-finder-rig` (key, store and state) was deleted after the
evidence was copied here. A scan of this folder for the run's key found nothing.
