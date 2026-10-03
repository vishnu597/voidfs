# Step 4: client core, CLI and Rust SDK

*Work items for step 4 of the [parity plan](PARITY.md#7-step-by-step-plan), written on 1 October
2026. The code references point at `main` at `939b57c`. What SpaceFS does is marked by how it is
known: **read** (their docs or the CLI's help), **observed** (run or looked at on this Mac, Space
0.2.333) or **inferred** (from strings, file names or a schema, not confirmed).*

Step 4 is **done when** a person can do from a terminal, and a Rust program through the SDK,
everything SpaceFS's CLI does without an account: list, create, fork and delete drives; read,
write and edit files; read and restore history; queue uploads that survive the terminal closing,
with pause, resume and a speed limit; and see one page of status. And when the client core the
mount (step 5) runs on exists and is tested: its cache, write journal, upload queue and change-feed
client. JSON output everywhere, as SpaceFS's CLI has.

## 1. What SpaceFS offers

### 1.1 The SDKs (read: docs.spacefs.com, `llms-full.txt`, 1 October)

- Four SDKs (TypeScript, Python, Go, Rust), version 0.3.0, MIT. Each wraps the official AWS SDK
  and adds the extensions. The Rust one is `spacefs-s3sdk`, imported as `s3sdk`: `Client::new(Config
  { access_key_id, secret_access_key, ..Default::default() })`, with `client.s3()` for any standard
  call (path-style).
- Calls: `list_drives`, `create_drive(name, CreateDrive { display_name, placement })`,
  `fork_drive`, `delete_drive(name, hard)`, `describe_drive`, `mount_credentials`; `put_object`
  (`PutOptions`: content type, `if_version`, `if_match`, `if_none_match_any`), `get_object`
  (`ReadOptions`: `version_id` or `as_of`, `consistent`), `read_range`, `write_at`, `patch`,
  `raw_patch`, `truncate`, `insert`, `remove_range`, `splice`, `rename`, `delete_object`,
  `list_objects` (every page), `list_versions`, `restore_version`, and the patch codec.
- **One error type**: `s3sdk::Error`, with `status()`, `code()`, `current_version_id()` (the head
  on a `412`) and the request id.
- **Retries:** reads, guarded writes, whole-object puts, offset writes and patches are safe to
  retry; "unguarded inserts and removals are not idempotent", so "never retry an unguarded insert
  or remove_range". The SDKs retry a fork's `503 SlowDown`.
- **Direct uploads** are opt-in and only in the Rust SDK (`direct_uploads: true`, bodies of 8 MiB
  and more): the client cuts the body into shards, asks which the drive lacks, PUTs those to
  presigned URLs and commits; it falls back to an ordinary put if either step fails with anything
  but `409` or `412`. "New bytes are usually faster through an ordinary put."
- **Mount credentials** (`?x-s3sdk-mount`): short-lived storage credentials for one drive's
  prefix, with `canWrite`, `accessGeneration`, a `storageBudget` the writer enforces, and a
  `backend` (for example `r2`); `501` where the deployment can't mint them.

### 1.2 The CLI (read: `spacefs --help` and every subcommand's, 0.2.333; observed: the read-only commands, and the daemon running in 0.2.343)

- `space 0.2.333 (8bebbc8, 2026-09-30)`: the app updated itself from 0.2.300 since the last
  stocktake. The CLI is `spacefs-fskitd`, which is also the mount daemon.
- Commands: `login`, `logout`, `whoami`, `drives` (`ls`), `drive create|delete|rename`,
  `workspace list|show|use`, `use`, `keys create|list|revoke`, `mount`, `unmount`, `mounts`,
  `uploads [cancel]`, `upload`, `status`, `daemon start|stop|status|info|restart|install|uninstall`,
  `history`, `show`, `restore`, `version`; hidden: `update [--check] [--json]`, `workspaces`. There
  is no `fork` command.
- `--json` on `drives`, `drive create`, `mounts`, `uploads`, `status`, `history`, `workspace
  list|show`, `keys list` and `update --check`; `keys create --format text|env|json`. Not on
  `version`, `whoami`, `daemon *`, `show`, `restore` or `unmount`.
- `upload <paths…> <bucket>:[/folder]` hands the transfer to the daemon ("it survives this command
  exiting"), with `--detach` to return at once. `uploads` shows a table, `--watch` redraws every
  second, `--json` gives the snapshot, and `uploads cancel <id> | --batch <id> | --all`.
- `history|show|restore --bucket <id> <path>`: `show` and `restore` take `--at` (RFC 3339 or Unix
  seconds) or `--version`; `restore` of a folder needs `--at`. They take the drive's id, not its
  name.
- `mount <drive> [mountpoint]` (default `~/Space/<Drive Name>`), `--read-only`, `--allow-other`,
  `--foreground`, `--adapter auto|smb|fuse|fskit|winfsp|nfs`, `--block-size` (8 MiB),
  `--cache-bytes` (64 MiB), `--local-ignore` (paths a `.gitignore` names stay on the Mac and are
  never uploaded), and about 25 `--retained-*` options of a "retained-v1 engine": a durable
  journal directory, a disk budget and reserve, a cache shared by every drive, read-ahead, an
  upload bandwidth ceiling in bytes per second ("the app's slider sends 0 for Unlimited"), 16
  objects published at once and 16 chunks of one file staged at once, a settle time before a
  file publishes (0), and files up to 8 MiB published "with the inode".
- `daemon info`: "the running daemon's build identity and write-durability counters"; its
  "restart safe" line is what the Mac updater checks, because bytes only in the daemon's
  write-back cache "are not durable anywhere".
- Run on this Mac (observed, 1 October): `whoami` and `drives` say "you are not signed in" (the CLI
  has no session; the app keeps its own); `status` and `status --json` report the daemon not
  running (`{"daemon":{"running":false,"socket":…/spacefs/fskitd.sock}}`); `mounts --json` gives
  `{"daemon":"not running","drives":[]}`; `uploads` fails, the daemon not reachable. **All of this
  while the app has a drive mounted**: the CLI sees only the FSKit daemon's socket, not the SMB
  mount below.
- The daemon's requests (inferred from strings): `uploads_pause`, `uploads_resume`,
  `uploads_cancel`, `uploads_status`, `list_upload_queue`, a bandwidth setter, `set_cache_dir`,
  `set_cache_limit`, `set_readahead`, and `set_pinned`, `pin_state`, `pins_changed`.

**The daemon, running (observed, 3 October, 0.2.343 (`403cbab`, 2 October), to which the app had
updated itself).** The app wasn't running and nothing was mounted; the CLI was signed in.
- `daemon start` took 0.1 s: it spawned `spacefs-fskitd --socket <state>/fskitd.sock --log-file
  <state>/spacefs-fskitd.log`, detached (its parent launchd, in a process group of its own), and
  said "daemon started (socket …)". Its help still says it spawns `spacefs-fused`. Run again, it
  says "daemon already running (socket …)", with status 0.
- The state directory (`~/Library/Application Support/spacefs`) became `0700`, the socket is
  `0600`, and `fskitd.sock.lock` holds the daemon's pid. The log is JSON lines: the build, the
  process (pid, parent, user, working directory), the disk cache
  (`~/Library/Caches/spacefs/blocks2`, at most 20,480 MB), and a diagnostic of the host (macOS
  version, SIP, the code signature, Gatekeeper's verdict, file limits) that took 328 ms.
- `daemon status`: "daemon running · up 5s · 0 drive(s) mounted · socket …".
- `daemon info`, text only: version, pid, uptime, socket, mounts, `restart safe YES`, and the
  counters behind it: `writeback` ("0 cached bytes, 0 pinned"), `dirty handles` (handles,
  bytes), `in flight` ("0 non-resumable, 0 total, 0 metadata"), `upload queue` (shards, bytes
  remaining) and `resumable` ("0 import(s) (source on disk + journaled)"). (Inferred: restart
  safe turns NO while write-back bytes or non-resumable requests are in flight.)
- `status --json` is `{"daemon": {"running", "socket", "uptime_secs"}, "mounts": [], "uploads":
  {…}}`, where `uploads` is the daemon's `uploads_status` answer, as `uploads --json` prints it:
  `uploads`, `batches`, `results`, `total_files`, `files_placing`, `files_preparing`,
  `remaining_bytes`, `queue_depth`, `workers_busy`, `workers_total` and `bandwidth_boosted`. As
  text: "daemon running · up 5s · socket …", "drives none mounted", "uploads idle". `uploads`
  says "no uploads in flight".
- `mounts --json` is `[]` while the daemon runs, and the object above while it doesn't.
- The socket doesn't speak HTTP (a `GET` gets an empty answer), and neither one JSON line nor a
  length-prefixed JSON message got an answer. (Inferred from strings: tagged JSON requests such
  as `ping`, `hello`, `daemon_info`, `list_mounts`, `mount`, `unmount`, `uploads_status`,
  `uploads_pause`, `uploads_resume`, `uploads_cancel`, `list_pending_uploads`,
  `set_upload_bandwidth`, `set_cache_limit`, `pins_status` and `shutdown`.)
- `daemon restart` took 0.4 s. `daemon stop` returns at once (8 ms) with "daemon stopping (volumes
  detach; remembered drives restore on next start)", without waiting for it, and leaves the
  socket and its lock file behind. Run again: "daemon not running", status 0.
- **`upload` needs a mounted drive, and doesn't start the daemon.** With the daemon running and
  nothing mounted, `upload --detach <file> <drive>:/void-probe/daemon` failed at once: "import
  failed — is <drive> mounted? (`space mounts`, then `space mount --bucket <drive>
  <mountpoint>`)", caused by "unknown session (the daemon restarted; remount required)". With the
  daemon stopped, it failed the same way, the daemon "not reachable" (the connection refused on
  the socket left behind), and started none. Nothing was uploaded; mounting the drive was
  outside what was approved, so not tried.
- `daemon install|uninstall` write a systemd user unit, "Linux only": on macOS, the app is what
  brings the daemon back.

### 1.3 The Mac app (observed through computer use, 1 October, 0.2.333)

- **The drive is mounted through a loopback SMB server, not FSKit.** The process is `spacefs mount
  --retained-v1 --adapter smb --bucket spacedb-…-data … /tmp/Space` with a 20 GiB disk budget and
  a 20 GiB cache shared by every drive; `mount` shows `//…@space.localhost:59123/Space on
  /private/tmp/Space (smbfs)`. Settings → Advanced → **Mount Method: SMB** says: "Drives mount from
  a private SMB server on this Mac, backed by a local journal… Uploads pause while a drive is
  ejected, including when Space quits, and resume when it mounts again." The **Filesystem Daemon**
  ("Serves FSKit volumes over the local socket") is **Not running**. Since 0.2.300 the app also
  runs `space-agent` and `space-computer-use` processes (inferred: its AI features).
- **Uploads:** "No uploads in progress", an Open Uploads view (search, upload files, activity),
  and **Upload Bandwidth**: a slider from Default ("Space adjusts upload speed to your connection
  automatically") to a cap to Unlimited; "SMB-mounted drives take this when they next mount." With
  nothing uploading, no pause or resume control showed; starting an upload would change the
  user's drive, so it wasn't tried.
- **Cache:** a disk cache, "16 KB of 20 GB used", with a Maximum Size slider ("What you haven't
  opened in a while is removed first; lowering the limit frees space now") and Clear Cache; an
  **in-memory cache** of 192 MB; a movable cache location; and **Offline Access: Pinned Files**
  ("Pinned files are never removed"). Pinning is in the voidfs plan as D8, after parity; SpaceFS
  now ships it.
- **Drives:** "1 drive · 1 mounted", New Drive, and the mounted drive with Eject.
- **The mount's local state** (file names and SQLite schema only, read-only):
  `retained-smb/mounts/<bucket>.json` (the remembered mount: pid, path, log offset),
  `cloud-cache/<sha256>` (content-addressed), and one `state.sqlite` per drive with tables for
  inodes (`objects`, `dirents`, `xattrs`, `holes`), locally written blocks (`chunks`), fetched
  blocks (`cache_blocks`, keyed by object, revision and block) and pins (`cache_pins`), dirty
  objects (`pending`, `mutation_log`) and the upload side (`publications`, `remote_queue`,
  `remote_ranges`, `remote_keys`). Inferred: the mount is a local filesystem in SQLite, published
  per object revision in the background.
- Not observed: Finder badges and context menus (the drive is empty, and browsing it in Finder
  writes `.DS_Store` into it), the menu-bar popover, and any offline state (the network was left
  alone).

### 1.4 Uploads and a folder restore (observed through computer use, 2 October, 0.2.333)

*In `void-probe/` on the trial drive, the one folder the user approved for these two
observations; it stays there. Measured from Bash: the mount's own SQLite state, read only, sampled
every second, and the Mac's bytes out on every interface but loopback. Times are UTC. The raw log
is not in the repository.*

**Two ways in, one queue (observed).**
- Writing through the SMB mount (`cp` into `/tmp/Space/void-probe/upload/`): a 512 MiB copy
  returned in 3 s, and 768 MiB in 4–6 s. The bytes land in the mount's local journal, and the file
  publishes in the background: the `pending` table holds it until its `publications` row is done.
- The app's own upload: the launcher's Upload Files (⌘N) searches indexed files only ("No files
  match" for a path under `/private/tmp`), then asks "Choose a drive for app-probe-1.bin…", then
  for a folder ("Search folders"), and ends with "Upload into upload ⏎". The app remembers the
  folder (`uploads.lastDestination` in its defaults). Uploads also start by dropping files on the
  menu-bar icon or the Settings window (1 October).
- An app upload is an import into the same journal, logged in `…-data-imports/operations.jsonl`
  (one JSON line with a SHA-256 per operation): a `Batch` with a conflict policy (`files:
  KeepBoth`, `merge_folders: false`; the app's strings offer Keep Both, Skip, Replace All Existing
  and Skip Identical Items), then each entry `Placing` → `Copying` → `AwaitingCloud` → `Done` or
  `Cancelled`, then `Covered` and `Finished` with the batch's totals. A 768 MiB file was in the
  journal within about a second, and then waited for the publisher like an SMB write. (Inferred:
  one publisher for both; an import is a local copy first.)

**The uploads view (observed).**
- Settings → Uploads: In Progress ("1 file · 53% · 27.2 MB/s · 9 sec left") with a row per file
  (name, size, drive, and "Saving…" or "Queued"), and Recent with a row per batch ("h2h · 11005
  files · Space · 1.07 GB", "app-probe-1.bin · 1 file · Space · 805.3 MB", "Cancelled"), each with
  a remove button, and Clear. SMB writes show in In Progress but never in Recent.
- While anything uploads, the menu-bar menu has a line for it ("2 files · 66% · 4.8 MB/s · 1 min
  left") and Open Uploads. The launcher's uploads view has the same rows, a search field, and an
  actions menu: Upload Files, Clear Recent, Upload Bandwidth, Report Bug.
- Two files copied in over SMB were first "Saving…" and "Queued", then both "Saving…".

**Pause and resume (observed).**
- Only app uploads can be paused, one at a time: the selected row's default action is "Pause ⏎",
  and its actions (⌘K) are Pause, Open Space, Show in Finder, Copy Name and Cancel Upload. An SMB
  write's actions are only Open Space, Show in Finder and Copy Name. The app's strings also have
  "Pause All Uploads" and "Resume All Uploads" in the menu (`pauseAllUploadsFromMenu`), which the
  menu didn't show while SMB writes uploaded (inferred: offered while an app upload runs).
- Pausing stopped the bytes within 2 s: a toast ("Paused "app-probe-1.bin""), a Paused section,
  and "Resume ⏎". 713 MB of the 805 MB had gone out. Resuming ("Resuming …", the row back at 64%)
  took 191 MB more: it continued, sending about 100 MB again (inferred: the chunks in flight).
- Settings says uploads to an SMB drive pause while it is ejected, including when Space quits.

**Cancel (observed).** Cancel Upload asks: "Cancel uploading "app-probe-2.bin"? Files that haven't
finished uploading are removed from the drive. Anything already uploaded stays." After it, the file
left the folder at once and the row said Cancelled, but the transfer went on: the whole file went
out (900 MB over the next 30 s), it was published as a version, and then its removal was
published. The import log says `files_cancelled: 1`, `bytes_transferred: 0`. (Inferred: cancel
removes the file from the mount's tree, and the publisher publishes that like a delete, after
whatever it already had in flight.)

**Quitting mid-upload (observed).**
- `osascript -e 'tell application id "com.spacefs.launcher" to quit'` is refused ("User canceled",
  -128): Space stays running and mounted, and closes its windows.
- Quit Space in the menu-bar menu asks nothing. The SMB mount was gone within a second, with 430 MB
  of an 805 MB file sent. Opened again, Space remounted `/tmp/Space` by itself in 6–8 s and
  finished the file with 507 MB more: it continued from the journal (inferred from the bytes; a
  restart would send at least 805 MB). The app stayed 0.2.333 (0.2.343 was waiting on "Restart to
  Update").

**Upload Bandwidth (observed).**
- The slider runs from Default ("Space adjusts upload speed to your connection automatically")
  through caps from 24 MiB/s ("Space keeps uploads near this rate, leaving headroom for the rest
  of your network") to Unlimited. A cap is saved at once (`uploadBandwidthTargetBps = 25165824`
  in the app's defaults; Default is the key's absence) and applies "when Space's file system
  service next starts": the next mount ran with `--retained-upload-bandwidth-bps 25165824`.
- Its effect couldn't be measured here: its lowest cap is above what this connection sustained
  uncapped (7–36 MB/s on average). Under the 24 MiB/s cap, a 768 MiB file went at 9.1 MB/s on
  average, with single seconds up to 44 MB/s (inferred: the cap is an average, not a limit on each
  second). The slider is back on Default and Space was restarted, so the mount runs uncapped again.

**Throughput and shape (observed).** A file published at 27–36 MB/s at best and 7–11 MB/s at worst
on this connection, with 12–14% more bytes out than the file (TLS, HTTP, and the re-sends above).
The mount records a published file as `remote_ranges` of about 7 MB each (78 for 512 MiB, 110 for
768 MiB), and each object's `remote_ref` as `{"group": "root-native", "record": <uuid>, "version":
<uuid>}`: a version id per object revision. A file `cp` wrote got two versions (inferred: the
content, then the modification time `cp` sets after it).

**Folder restore (observed, in part).** In `void-probe/restore/`: `a.txt`, `b.txt`, `c.txt` and
`sub/d.txt` written at 14:04:48 (published 14:04:56); at 14:05:39 `a.txt` and `sub/d.txt` changed,
`e.txt` added and `c.txt` deleted (published 14:05:44). The restore point was 14:05:24.
- The app has no history or restore view: neither its windows nor its strings have one, and the
  Finder extension only pins and unpins. The CLI's `restore --at` (which "rolls the whole subtree
  back" for a folder) needs `space login`, and it ignores an access key in the environment ("no
  config … run `space login` first"). The user signed it in.
- With an access key the user minted in the web app, through the S3 API: histories are per
  object, oldest first, with milliseconds (`a.txt`: `put` then `write`, the second `isLatest`),
  and a deleted file has none (`c.txt` answers `404 NoSuchKey`; there is no recently-deleted
  listing). Each object's version ids are the ones in the mount's `remote_ref`, and the ETag is
  `"v-<versionId>"`: from the version, not the content.
- `POST …/void-probe/restore/?x-s3sdk-restore` with `x-s3sdk-as-of` is `400 InvalidArgument`,
  "versionId is required": the API restores by version only.
- `POST …/void-probe/restore/?x-s3sdk-restore&versionId=<the folder's put>` answered `200` with a
  new version of the folder (`restore`, `restoredFrom` the put, size 0) and changed nothing in it:
  `a.txt` and `sub/d.txt` kept their writes, `e.txt` stayed, `c.txt` stayed deleted. Restoring a
  folder by version restores the folder object alone, as voidfs's does.
- The folder's version in the mount's state after its entries changed (`eab883e3…`) is not in the
  API's history of the folder, even with `x-s3sdk-all=true` (unexplained).
- Inferred from the CLI's strings: its folder restore works on Space's engine directly
  (`RESTORE`, `GROUP_MOVE_IN`, `GROUP_MOVE_OUT`, `restore_from_version`, transactions with a
  `txn_key`), with the signed-in session's credentials, not through the S3 API.
- The CLI's `history --json` is not the API's: each entry is `{conflict, created, op
  (create|update|restore), size_bytes, version}`, and it showed only versions from about
  14:05:43 on: `a.txt` its write but not its create, `b.txt` and `sub/` nothing, and the folder
  the `eab883e3…` version (an `update` when its entries changed) and the API's restore. (Inferred:
  it reads Space's engine log since its last checkpoint.)
- `spacefs restore --bucket <drive> void-probe/restore --at 2026-10-02T14:05:24.462Z` printed
  "restored 3 record(s) under 'void-probe/restore' to their state at …" and no version id. After
  it, **the whole folder was gone**: the API answers `404` for the folder and everything in it
  (the listing of `void-probe/restore/` is empty, and the old versions by id are `404
  NoSuchVersion`), the CLI says "no such path" for the folder and each file, and the mount
  caught up over the next minutes. (Inferred: the CLI found no version of the folder or of
  `a.txt` at the restore point in the history it reads, took them as not existing then, and
  removed them; the folder took its subtree with it.) So no restored file could be looked at, and
  whether a working subtree restore gives each file a `restore` version, under one id, stays
  unknown. What's left of `void-probe/restore/` stays as it is.

**For the client core (item 3)**, where these differ from item 3's design:
- One queue behind two ways in, as Space has: the journal's publisher, and imports that a client
  hands it. Item 3 already designs it so.
- Space pauses and cancels app uploads one at a time, and SMB writes not at all (but by ejecting).
  voidfs keeps pause and resume globally, per drive, per batch and per item, for writes and
  imports alike.
- Space's cancel removes the file and lets the upload finish. voidfs's cancel aborts the transfer
  and never publishes the file: no upload is wasted and no stray version made. An import that has
  not published leaves nothing in the drive.
- Space's cap applies at the next mount and starts at 24 MiB/s. voidfs's token bucket applies at
  once and takes any rate.
- Space resumes after a quit from its journal, as item 3 designs voidfs's queue to.
- Space's imports have a conflict policy (Keep Both by default). The CLI's `upload` makes a new
  version of a file that exists instead, since every version is kept; the queue keeps that, and a
  policy can come with item 4's `upload` if it is wanted.

## 2. What voidfs has

- **The protocol**, all of it served by `voidfs-server` but direct upload (§4.11, not
  implemented) and storage credentials (§5.5, which answer `501`). Display names and copy-mode
  forks aren't implemented either (PARITY §3).
- **Request signing** for the extensions in `crates/voidfs-conformance/src/runner.rs`
  (`aws-sigv4` plus `reqwest`), and `aws-sdk-s3` 1.148.0 in the benchmark.
- **The engine** in `voidfs-core`: the chunker, the patch codec, ids and timestamps, which a
  client reuses as they are.
- **The Mac spike** (`apps/macos`): a read-only FSKit mount with its own Swift SigV4 client and a
  menu-bar shell. No Rust client code, no CLI, no SDK.
- `voidfs-server` is a binary only, so nothing outside it can start one in-process for a test.

## 3. Order

1. **The Rust SDK** (A1, Rust): every protocol call, typed. It runs on the protocol as it is, so
   it is usable at once, and everything after builds on it.
2. **The CLI on the protocol** (D9), whose command is `void`: `drives`, `drive`, `fork`,
   `history`, `show`, `restore`, `version`, and a foreground `upload`, with `--json` everywhere.
3. **The client core** (D1): the disk cache, the write journal, the upload queue and the
   change-feed client, in `crates/voidfs-client`.
4. **The daemon and the CLI's daemon commands** (D1, D9): `daemon`, `upload` (handed to the
   daemon), `uploads`, `status`, and the mount table `mount`, `unmount` and `mounts` will use.
5. **Direct uploads** (E10, protocol §4.11), server and client.
6. **Short-lived storage credentials** (B4, protocol §5.5), server and client.

Why this order: 1 and 2 run on the protocol that exists and give a usable slice soonest. The
client core is P0 and the mount needs it; direct uploads and credentials are P1 and make it
cheaper, not possible, so the core is built so that they slot in (its fetcher and its uploader
are interfaces). 5 comes before 6 because the upload queue gains from it at once, while reading
straight from the bucket needs the most new code (a read-only reader of the format in the
client) and decisions only the user can make (§5).

## 4. Work items

### Item 1. The Rust SDK

**Checklist:** A1 (the Rust half; TypeScript waits for step 7).

**Status (1 October 2026): done**, as designed below. What was built:
- `crates/voidfs-sdk`: `Client` and `Config` (or `Config::from_env`), the `aws-sdk-s3` client
  underneath as `client.s3()`, and a call for every protocol request but direct upload: the drive
  calls of §5, `put_object`, `get_object`, `get_object_stream`, `head_object`, `read_range`,
  `delete_object`, `list_objects`, the edits of §4.1–§4.3, `rename`, `list_versions`,
  `restore_version`, `restore_as_of`, `attributes`, `set_attributes`, `list_folder` (and a page at
  a time), `list_deleted`, `changes` and `watch_changes`.
- `Error`, with `status()`, `code()`, `current_version_id()` and `request_id()`, for the SDK's
  own requests and the AWS client's alike; `Transport` errors say whether the request may have
  reached the server.
- The retry rule, in `retry.rs`, as below.
- `voidfs_sdk::sign`, the signing code the conformance runner had, which the runner now uses.
- `voidfs-server` as a library too, with `voidfs_server::test_server::TestServer`: a server over a
  memory pool in the test's own process.

What it showed about the server, for later: the event stream ignores `Last-Event-ID` (the client
resumes with `since`, which works), and `x-voidfs-display-name` is ignored (PARITY §3).

**Design.** A crate `voidfs-sdk` (`crates/voidfs-sdk`):
- `Client::new(Config { endpoint, access_key_id, secret_access_key, .. })` or
  `Config::from_env()` (`VOIDFS_ENDPOINT`, `VOIDFS_ACCESS_KEY_ID`, `VOIDFS_SECRET_ACCESS_KEY`, the
  names the server takes its admin key from). `client.s3()` is the official `aws-sdk-s3` client,
  path-style, signed with the same key, for anything standard (multipart, copy, tagging).
- **Typed calls for every extension**, each a method with an options struct that is
  `Default`:
  - drives (§5): `list_drives`, `create_drive` (with a display name), `fork_drive`,
    `delete_drive` (soft or hard), `undelete_drive`, `describe_drive`, `storage_credentials`;
  - objects (§3, §4): `put_object`, `get_object`, `get_object_stream`, `head_object`,
    `read_range`, `delete_object`, `list_objects` (every page, through `aws-sdk-s3`);
  - edits (§4.1–§4.3): `write_at`, `truncate`, `patch` (the codec from `voidfs-core`), `splice`,
    `insert`, `remove_range`;
  - history (§4.4–§4.6): `list_versions`, `restore_version`, `restore_as_of` (a file, or a
    folder's subtree);
  - the rest of §4: `rename` (with `replace`), `attributes`, `set_attributes`, `list_folder`
    (§4.9, with its `seq`), `list_deleted` (§4.10);
  - the change feed (§5.6): `changes` (long poll) and `watch_changes` (Server-Sent Events, which
    reconnects from the last `seq` it delivered and reports `410 ChangesExpired` to the caller).
- Preconditions (`if_version`, `if_match`, `if_none_match_any`) and the attribute headers
  (`mtime`, `mode`) wherever the protocol accepts them. Results carry `version_id`, `etag` and
  `size`; a `412` carries the current version.
- Timestamps stay the strings the server sent (`lastModified` with microseconds), so they pass
  unchanged to `as_of`, as §4.4 asks.
- **One error type**, `voidfs_sdk::Error`: `Service` (status, code, message, request id, current
  version on a `412`), `Transport` (with whether the request may have reached the server), `Decode`
  and `Invalid` (refused before sending). Errors from `aws-sdk-s3` map into it.
- **The retry rule.** Extension calls retry `500`, `502`, `503` and `504`, timeouts and connection
  failures, three attempts by default with jittered backoff, honouring `Retry-After`. **An
  unguarded splice (insert or remove) is never retried once it may have reached the server**:
  only a failure to connect, which sends nothing, is retried. A rename without a precondition is
  treated the same way, since a retry after it landed could move a newer object that took the
  source's name. Everything else is idempotent in effect (§6 of the protocol), and `412` is never
  retried: the caller re-reads from the version it names.
- **Signing** moves from the conformance runner into the SDK (`voidfs_sdk::sign`), which the runner
  then uses, so there is one copy. It keeps what the runner needs: unsigned headers on purpose, and
  virtual-host addressing.
- **Testing in-process:** `voidfs-server` gains a library target, and with it
  `voidfs_server::test_server`, a server over a memory pool on a free port, for the SDK's tests and
  later the CLI's and the client's.

**Out of scope here:** direct uploads (item 5), which the SDK gains with the server side;
`placement` and `consistent`, reserved in protocol 1.

**Checked by:** tests against the in-process server for every call; a fault-injecting proxy that
answers `503`, or forwards a request and then holds the answer past the client's timeout, to show
that idempotent calls retry and succeed and that an unguarded splice is applied once and not
retried; each test seen to fail with the code it guards broken; the conformance suite through the
moved signing code on memory, local disk and versitygw.

### Item 2. The CLI on the protocol

**Checklist:** D9.

**Status (1 October 2026): done**, as designed below, with the decisions after it. What was built:
- `crates/voidfs-cli`, whose binary is **`void`**, on the SDK. The user chose the name on 1 October:
  the command is `void`, while the project, the crates and the server keep `voidfs`.
  - `void drives` (also `ls`), `void drive create|show|delete|undelete` and `void fork`;
  - `void history`, `void show` and `void restore`, for a file or a folder;
  - `void upload`, in the foreground;
  - `void version` and `void keys generate`.
- `--json` on every command, errors as JSON on stderr, and exit statuses: 1 for an error, 2 for a
  usage error.
- In the SDK: errors of `client.s3()`'s calls convert into `voidfs_sdk::Error` with `?`, and the
  crate re-exports `aws_sdk_s3`, for the types of those calls.
- In the server, a fix the CLI needed: the recently-deleted listing (protocol §4.10) missed an
  object whose key was the whole `prefix`. Its range started at the prefix paired with the id
  `root`, which sorts after every `o-…` id, so it began past that key's rows.

**Design.** A crate `voidfs-cli` with the binary `void`, on the SDK:
- `void drives [--json]`: alias, id, created, size (describe for each, a few at a time).
- `void drive create <name> [--display-name] [--json]`, `drive delete <name> [--hard] [-y]`
  (asks for the name unless `-y`, as SpaceFS's does), `drive undelete <name>`, `drive show
  <name> [--json]`. SpaceFS's `drive rename` renames a display name, which voidfs doesn't store
  yet (PARITY §3, S3), so it waits for that.
- `void fork <source> <name> [--json]`: SpaceFS has no command for it, but forks need no
  account, and agents use them.
- `void history <drive> <path> [--all] [--json]`, `void show <drive> <path> [--at <time> |
  --version <id>] [-o file]` (to stdout by default, streamed), `void restore <drive> <path>
  [--at <time> | --version <id>] [--json]`. `--at` takes RFC 3339 or Unix seconds, as SpaceFS's
  does. A drive is named by alias or id everywhere, where SpaceFS's three take only the id.
- `void upload <paths…> <drive>:[/folder]`: in this item it runs in the foreground (multipart
  through `client.s3()` from 64 MiB), with progress on stderr and a JSON summary with `--json`;
  item 4 hands it to the daemon.
- `void version [--json]`; `void keys generate [--scope read|write|admin] [--format
  text|env|json]`, which makes a key for `voidfs-server --key`: the self-hosted counterpart of
  `keys create`, which needs no account.
- Configuration from the SDK's environment variables, or `--endpoint` and a key file. `--json`
  prints one JSON document on stdout and errors as JSON on stderr, with the exit status set.

**Decisions taken while building it:**
- **Configuration.** `--endpoint`, `--access-key-id`, `--secret-access-key` and `--key-file` (or
  `VOIDFS_KEY_FILE`), on every command. A key file is what `keys generate --format json` or
  `--format env` prints. A flag wins over the key file, which wins over `VOIDFS_ACCESS_KEY_ID` and
  `VOIDFS_SECRET_ACCESS_KEY`. `VOIDFS_REGION` is read as the SDK reads it. `version` and `keys
  generate` need none of it.
- **Output.** `--json` gives one JSON document on stdout, pretty-printed. An error is
  `{"error": {"code", "status", "message", "requestId", …}}` on stderr: the code is the server's
  S3 code, or one of the CLI's own (`Usage`, `NoCredentials`, `InvalidArgument`, `RequestFailed`,
  `UnexpectedResponse`, `LocalFileError`, `NotConfirmed`). A `412` adds `currentVersionId`, a
  transport failure `sent`. Usage errors are JSON as well when `--json` is on the command line.
  A stdout closed early (`| head`) ends the command quietly, with status 0.
- **`drives`** describes each drive, 8 at once. Its JSON is `{"drives": […]}`, each as `drive
  show --json` gives it. A drive deleted between the listing and its description is left out. A
  fork's source is shown by name when the key reaches it.
- **`drive delete`** reads the drive's name back on stdin unless `-y`; no answer is no. With
  `--json` the question isn't printed, so that stderr holds only JSON, but the name is still
  read. **`drive undelete`** takes the alias: the server finds deleted drives by alias only.
  `--display-name` is sent, and the server ignores it for now (PARITY §3, S3).
- **`history` of a folder**, named with or without its `/`, lists every version of every file in
  it, oldest first. Each time is one `restore --at` takes. It comes from each file's
  `?x-voidfs-versions`, 8 at once, not from `ListObjectVersions`: the times that gives have
  milliseconds only, so one passed to `--at` could fall before the version it names. Files
  deleted since aren't listed.
- **A deleted file or folder.** `history`, `show` and `restore --at` say when it was deleted, and
  give the command that brings it back: `void restore <drive> <path> --version <lastVersionId>`
  (protocol §4.10), quoted for a shell. Its JSON error carries the `deleted` entry.
- **`show`**: `--at` is sent as RFC 3339 in UTC with microseconds, and a time before the file
  existed says so. `-o` fills a file beside the target and renames it into place, so a failed read
  leaves the target as it was; `-o -` is stdout. `--json` needs `-o`, since otherwise the content
  is what goes to stdout; it then prints what was written.
- **`restore`** needs `--at` or `--version`. A folder named without its `/` is found. `--version`
  also brings back a deleted file or folder (§4.10).
- **`upload`**:
  - a folder goes up under its name, with everything under it; files keep their modification time
    and permission bits (`x-voidfs-mtime` and `x-voidfs-mode`, on a multipart upload's first
    request); empty folders are made; symbolic links inside folders are skipped, with a warning;
  - one put below 64 MiB; from 64 MiB, a multipart upload in 16 MiB parts (larger past 156 GiB, to
    stay within 10,000 parts), 4 parts of a file at once;
  - 16 files at once (SpaceFS's daemon's figure), within 128 MiB held in memory: each put and each
    part waits for its share, and runs as its own task once it has it;
  - it stops starting files at the first failure, aborts a multipart upload that failed, and
    reports what was uploaded, skipped, failed and not attempted, then exits 1. Each file is its
    own version, so running it again uploads everything again;
  - progress is one line on stderr, rewritten, when stderr is a terminal.
- **`keys generate`**: `--scope` (or `--access`, SpaceFS's name), `write` by default as SpaceFS's;
  `--format text|env|json`, and `--json` means `--format json`. The id and secret are drawn as the
  server draws its own. The text gives the `--key id:secret:scope` value, `env` prints quoted
  `export` lines, and the JSON carries `serverKey`.
- **`version`** prints `void 0.0.0 (<commit>, <date>)`, as SpaceFS's prints `space 0.2.333
  (8bebbc8, 2026-09-30)`, from git at build time (`unknown` without git); `-V` says the same.
  `--json` adds the protocol version, 1.

**Where it differs from SpaceFS's CLI, and why:**
- A drive is named by alias or id everywhere. `history`, `show` and `restore` also take SpaceFS's
  `--bucket <drive> <path>`; `--root-group` has no counterpart.
- `fork` exists; SpaceFS has none.
- `--json` is on every command, `show` (with `-o`), `restore`, `fork` and `version` included, and
  errors are JSON with it. SpaceFS's errors are text even with `--json` (observed, 1 October), and
  its `version`, `show` and `restore` have no `--json`.
- `keys generate` makes a key on this machine for `voidfs-server --key`, where SpaceFS's `keys
  create` asks the account. It has no `--drive` or `--label`: the server has no flag for a drive
  allowlist yet (PARITY §3, C3). `keys list|revoke` wait for accounts.
- `upload` runs in the foreground, without `--detach`, until the daemon (item 4).
- `drive create` has no `--backend`: a deployment has one pool.
- Not here: `drive rename` (display names, above); `login`, `logout`, `whoami`, `workspace`,
  `use` and `keys list|revoke` (step 6); `update` (D10); `mount`, `unmount`, `mounts`, `uploads`,
  `status` and `daemon` (item 4, step 5).

**What it showed about the server** (not changed in this item: each changes what replay derives
and checkpoints store; both are fixed since):
- **A folder restore** (§4.6) gave each file it restores the folder's new version, but a
  transaction wrote one history row, for its target, so the files' histories didn't list that
  version: `isLatest` marked an older row, and `show --version` with the file's current version
  answered `404 NoSuchVersion`. The folder's own HEAD kept its old version, while its history
  listed the restore. Format §8.2 says `history` holds every version of every object. Fixed by
  [RFC 0004](../rfcs/0004-a-version-for-every-object-it-changes.md), in a pool with the
  `multi-object-versions` feature: the restore's version is the folder's and every changed
  file's, each with its own row.
- **`lastVersionId`** (§4.10) of an object that a folder restore removes, or that a rename with
  `x-voidfs-replace` replaces, was the head of the transaction's target, not the object's own:
  bringing it back with that id failed, or brought back the wrong object. Fixed in
  [#23](https://github.com/vishnu597/voidfs/pull/23): the object's own head.
- `ListObjectVersions` gives times in milliseconds (above), and `undelete` finds deleted drives
  by alias only.

**Out of scope:** `login`, `logout`, `whoami`, `workspace`, `use`, `keys list|revoke` (accounts,
step 6); `update` (self-update, D10).

**Checked by:** the binary run against the in-process server in integration tests, for each
command in text and JSON and its errors, each test seen to fail with the code it guards broken;
and the README's commands run by hand against a server.

### Item 3. The client core

**Checklist:** D1 (with D6's cache and D8's pins started).

**Status (2 October 2026): done**, in three pull requests: the cache and fetcher, the journal and
upload queue, and the change-feed client and connectivity. What the first built:
- `crates/voidfs-client`, with `Store` (the per-user `state.sqlite`), `Cache`, `Reader` and the
  `Fetch` trait with `ApiFetcher`, as designed below.
- In the SDK, `get_range`: a ranged read that also returns the version's headers, so that the
  fetcher checks it got the version it asked for.
- [`bench/scripts/client-randread.sh`](../bench/scripts/client-randread.sh) and the `randread`
  example, which measure it as the head-to-head measured the apps
  ([bench/results/client-cache](../bench/results/client-cache/README.md)): random 4 KiB reads at
  p50 0.06–0.15 ms cold (R2 included) and 0.03–0.05 ms warm, where the spike took 140–164 ms and
  Space 1.2 ms; and a reader that waits for each 1 MiB read streams at 37–40 MB/s from R2 (Space's
  Finder copy: 36–38) and 760 MB/s through the 12 ms relay, where reading directly gives 9–11 and
  56.

**Decisions taken while building it:**
- **SQLite:** `rusqlite` 0.40 with its `bundled` feature (SQLite compiled in, so the Mac's system
  SQLite doesn't matter). It declares no minimum Rust; it builds here on 1.98, and the APIs this
  crate uses are older than 1.90 (checked by hand: `File::try_lock` 1.89, `as_chunks` 1.88). The
  database is WAL with `synchronous = FULL`, for the journal's durability. One process owns a state
  directory: `Store::open` takes an exclusive lock on `<dir>/lock`. The state directory is
  `~/Library/Application Support/voidfs` on macOS, `$XDG_STATE_HOME/voidfs` elsewhere, and
  `VOIDFS_STATE_DIR` overrides both.
- **Blocks:** a file per block under `<state>/cache/`, named by the SHA-256 of drive, ETag and
  block number. A miss fetches the whole block (8 MiB, or the file's end), as SpaceFS's 8 MiB
  blocks do; a reader of many large files' first bytes pays 8 MiB each, which step 5 can tune.
- **Verified as filled:** through the API, the fetcher checks the response's version id and ETag
  against the ones asked for, and the cache checks the length. Checking the bytes themselves against
  the format's shard hashes needs an object's shard list, item 6's second open decision. On disk,
  each block's CRC-32C per 64 KiB is in the index and is checked on every read of those 64 KiB: a
  block that fails, or whose file is gone or the wrong size, is dropped and fetched again.
- **Limits:** 20 GiB at most and 5 GiB free on the volume at least (SpaceFS's reserve isn't
  known). A fill evicts what was used longest ago, a read from memory counting as a use; pinned
  content (by drive and ETag) never goes. When nothing more can go, the block isn't stored and is
  served from memory. `trim` applies both limits now (for the daemon, when the volume fills from
  elsewhere); `clear` removes all but pinned content, as SpaceFS's Clear Cache does. Use times are
  written to the index at most once a minute per block.
- **Memory:** 192 MiB of the blocks fetched most recently. A random read found on disk reads only
  the 64 KiB pieces it needs and doesn't load the block into memory.
- **Fetching:** one fetch per block however many readers wait (a fetch runs as its own task, so a
  reader that gives up doesn't stop it), a failure reaches every waiter and the next read tries
  again, and at most 16 fetches at once, of which at most 8 read ahead.
- **Read-ahead:** per `Reader`. A read is forward if it starts at or after the last one's start and
  no more than a block past its end (the kernel's reads of a stream overlap and arrive a little out
  of order); each forward read doubles how far ahead the reader fetches, from one block to 64 MiB.
- **Where item 6 slots in:** `Fetch` is the interface: a fetcher that reads shards with storage
  credentials implements it, and the cache's key would gain a shard-by-hash form. Neither is needed
  for the API path, so neither decision item 6 waits on was needed here.

What the second built (the journal and the upload queue):
- `Queue`, with `journal` and `publish`: the journal's calls (`put`, `write`, `truncate`,
  `rename`, `delete`, `folder`, `set_attrs`), each durable when it returns; `import` of files of
  the user's as a batch; `pause`, `resume` and `cancel` for everything, a drive, a batch or one
  entry; `set_bandwidth`; `status`, with items and batches; `close`.
- In the SDK: `Bandwidth`, a shared token bucket that paces request bodies 64 KiB at a time
  (`Config::upload_bandwidth`), and multipart calls on the SDK's own requests
  (`create_multipart_upload`, `upload_part`, `complete_multipart_upload`, `abort_multipart_upload`,
  `list_parts`), so that the limit covers them.

**Decisions taken while building the second:**
- **One queue, two ways in,** as Space has (§1.4): the journal's changes, and imports. The journal
  copies a change's bytes into `<state>/journal/` and syncs the file, its folder and its entry
  before the call returns. An import is read where it is, when it goes up; its size and
  modification time are recorded, and its multipart upload starts again if they change.
- **Order:** one key's changes publish in order, and so do a folder's and the changes under it,
  and a rename's and the changes to either name. Other keys go at once, 16 at a time.
- **Coalescing:** a put takes the writes and truncates after it into one put (below 64 MiB);
  writes take the writes after them into one patch (at most 64 MiB and 10,000 edits), with a
  truncate only last. Each change is guarded by the version it was based on; a change made on top
  of one not yet published is guarded by the version that one gets.
- **The `412` rule:** the local version is published anyway and the version it replaced is
  recorded as the conflict: a put is put again unguarded, a patch's local version (its base with
  its edits) is built and put, a rename, delete or attribute change is sent unguarded.
- **Its own lost answers:** each put carries `x-amz-meta-voidfs-entry` (the state's id and the
  entry's). A put whose answer was lost, retried into a `412`, or found in flight at a restart, is
  recognised as its own and not published twice. Other changes can't be told apart that way: a
  guarded patch retried after it landed gets a `412`, and its local version is published again
  as a second version with a conflict that wasn't one. A rename or a delete that landed is
  recognised from the object being gone.
- **Large files:** from 64 MiB, in 16 MiB parts (larger past 156 GiB), 4 parts of a file at once,
  within 256 MiB of bodies in memory across all uploads. The upload's id and each finished part
  are recorded; after a pause or a restart the parts the server still has, by `ListParts`, are not
  sent again. Pausing stops the parts in flight, which are sent again, as Space's resume does.
- **Pause, resume, cancel:** for everything, a drive, a batch or one entry, kept across restarts.
  Space pauses only app uploads, one at a time (§1.4). A cancel stops the transfer, never
  publishes, cancels the changes made on top of it, and aborts a multipart upload: Space's cancel
  removes the file but lets its upload finish and publish (§1.4).
- **Bandwidth:** a token bucket in the SDK, shared by the queue's requests, in 64 KiB pieces with
  a quarter of a second's burst; a new limit applies to the next piece, and is kept across
  restarts. Space's applies at the next mount and starts at 24 MiB/s (§1.4).
- **Failures:** a server or network failure the SDK has already retried is tried again after 2,
  4, 8 … seconds, at most a minute; another failure (a `409`, a `403`) waits for a resume. A
  failed change holds back the later changes to its key.
- **Attributes:** an import keeps its modification time and permission bits (on the put) and its
  extended attributes (as an attribute version after it), but `com.apple.quarantine`,
  `com.apple.provenance`, `security.*` and `system.*`. Space's uploader drops extended attributes
  (head-to-head). A file that exists becomes a new version, as `void upload` does, where Space's
  imports keep both by default.
- **Restart safe** (Space's `daemon info`): every change is synced before its call returns, so
  nothing unpublished is only in memory; `status` reports what is unpublished and its bytes, for
  the daemon's `info` (item 4).

What the third built (the change-feed client and connectivity):
- `FeedWatch`: a drive watched from a listing's position, its changes as `Invalidation`s.
- `Connectivity`: online, degraded or offline, with `check`, `watch` and `probe`.
- In the SDK, `Config::observer`: an `Observe` that is told what every attempt of the SDK's own
  requests came to (the status, or none, and whether it may have been sent).

**Decisions taken while building the third:**
- **What a change makes stale:** an `Object` (its attributes and content, and its entry in its
  folder's listing) for each key a change names, and its `fromKey` too (a rename's source, or
  where a folder restore moved an object back from); a `Subtree` as well for a folder renamed,
  deleted or restored, which the feed reports as one change for the folder (protocol §5.6); and
  `All` after a relisting. The block cache needs none of it: its blocks are keyed by ETag. Under
  RFC 0004 a folder restore also reports each object it changes; the subtree is stale all the
  same.
- **Positions:** a watch starts from the `seq` of the listing its caller holds. The SDK reconnects
  a broken stream from the last position it delivered (with `since`; the server ignores
  `Last-Event-ID`, item 1). On `410 ChangesExpired` the watcher relists only the root's first page,
  for a current position, says `All`, and watches on from there: the mount relists what it shows,
  as it is asked for, rather than the whole drive at once.
- **Connectivity from what requests get:** three requests in a row without an answer (refused,
  reset or timed out) make the link offline; any answer makes it online; three server errors
  (`5xx`, `429`) or answers slower than 10 s within 30 s make it degraded. The feed's stream counts
  too. It doesn't read the operating system's network state; the Mac app can add that in step 5.
- **Offline:** a block the cache doesn't have fails at once with `Error::Offline` (cached blocks
  are still read), and the queue holds its publishes. A probe (`ListBuckets`, 2 s after going
  offline, then twice as long each time up to 30 s) finds the server again, and the queue goes
  on.

**Design.** A crate `voidfs-client`, which the daemon (item 4) and the mount (step 5) run. One
SQLite database per user (`state.sqlite`, WAL, in the platform's application-support or state
directory, as SpaceFS keeps one per drive) holds the journal, the queue and the cache's index;
content lives in files beside it.
- **Cache.** Fetched content on disk, verified as it is filled, under a size cap (default
  20 GiB, as SpaceFS's) and a free-space floor: below the floor it evicts, and it stops filling
  rather than fill the disk. What was used longest ago goes first; pinned entries never do.
  Through the API, entries are blocks of a content version, keyed by drive, ETag and block (8
  MiB, SpaceFS's block size), which never go stale because an ETag names one content. With
  storage credentials (item 6) entries become shards keyed by hash, shared by every drive and
  fork. A small memory tier sits in front, as SpaceFS's 192 MB one does.
- **Write journal.** A mutation is durable on the Mac once its bytes and its entry are written and
  synced; that is when a save returns. Entries are per drive and key, in order: put (from a staged
  file), write at an offset, truncate, rename, delete, folder, attributes. Consecutive writes to a
  file coalesce (offset writes into one patch, a whole rewrite into one put) before they publish.
  Each publishes guarded by the version it was based on. On a `412` the local version is
  published anyway, since every version stays in the history, and the conflict is reported: no
  write is ever lost, which is D8's rule. "Restart safe" (SpaceFS's `daemon info`) is "the journal
  holds nothing unpublished that is not synced".
- **Upload queue.** The journal's publisher, and bulk imports: queued, uploading, verifying,
  done, failed or cancelled, with progress per item and per batch, persisted so that a restart
  resumes (a multipart upload keeps its id and finished parts). Pause and resume, globally and
  per drive; cancel by item, batch or all; 16 objects at once (SpaceFS's default); a bandwidth cap
  as a token bucket on request bodies, from unlimited down. SpaceFS's "Default" adapts to the
  connection; voidfs starts with unlimited and a fixed cap, and adapts later if measurement shows
  a need.
- **Change-feed client.** Per watched drive, the SDK's `watch_changes` from the `seq` of the last
  listing, reconnecting with backoff; on `410` it relists (§4.9) and resumes. Changes become
  invalidations of cached listings and attributes, which the mount turns into the kernel's.
- **Connectivity.** Online, degraded or offline, from the outcome of requests and the feed's
  liveness, so that the mount fails fast when offline (step 5) rather than hang.

**Checked by:** unit tests of each part against the in-process server and a fault proxy: cache
eviction under the cap and the floor, pins kept, corrupt cache files refetched; journal replay
after a simulated crash at each step, coalescing, and the `412` rule; queue pause, resume,
cancel, the cap's rate (never faster, and at most a third slower), and resumption after restart; feed reconnection and the `410`
relist.

### Item 4. The daemon and the CLI's daemon commands

**Checklist:** D1, D9.

**Status (3 October 2026): in progress**, in three pull requests: the daemon and its socket,
uploads through it, and the mount table with the launchd agent. What the first built:
- `crates/voidfs-daemon`: `Daemon`, which owns the state directory (the `Store`, the cache, the
  upload queue and connectivity) and answers on its socket; `api`, what the socket speaks;
  `DaemonClient`, a client for it; and `Settings`, the connection a daemon is left with.
- `void daemon run|start|stop|restart|status|info` and `void status`.

**Decisions taken while building the first:**
- **A crate of its own,** `voidfs-daemon`, which the `void` binary runs (`void daemon run`), as
  `spacefs-fskitd` is both Space's CLI and its daemon. The CLI talks to it through its
  `DaemonClient`; the tests start it in their own process as well as through the binary. The
  feed watches come with the mounts they serve (the third pull request).
- **The socket:** `<state>/daemon.sock`, HTTP/1.1 with JSON under `/v1/` (axum 0.8 serves a
  `tokio::net::UnixListener`; the client is reqwest 0.13's `unix_socket`): `GET /v1/status`,
  `GET /v1/info`, `POST /v1/stop`, and errors as `{"error": {"code", "message"}}`. Space's
  socket isn't HTTP (§1.2); voidfs's is, so that the Mac app and `curl --unix-socket` speak it
  too. Only its user may use it (`0600`), and the state directory is made `0700`, since it holds
  copies of the user's bytes and the daemon's key: Space does the same.
- **One daemon per state directory:** it holds the store's lock. A socket nothing answers on was
  left by a daemon that died, and is replaced once the lock is held. Stopping removes the socket
  and only then lets go of the lock, so that a daemon starting meanwhile keeps its own socket; a
  daemon that starts waits up to 2 s for one that is stopping.
- **The path's length:** a Unix socket's path may have 103 bytes on macOS (107 on Linux). The
  default, `~/Library/Application Support/voidfs/daemon.sock`, is about 60; a longer one is
  refused with what to do (set `VOIDFS_STATE_DIR` to a shorter directory).
- **The daemon's connection:** `daemon start` takes the server and key as every command does
  (flags, key file, environment), keeps them in `<state>/daemon.json`, readable by the user only,
  and launches the daemon with the connection variables taken out of its environment, so that it
  reads that file. A daemon launchd starts at login then needs no secret in its plist. `daemon
  run` uses a connection it is given, else the file. Space's daemon uses the session `space
  login` keeps in its `config.json`; the Keychain can hold the key once accounts exist (step 6).
- **`daemon start`** launches `void daemon run` in its own process group, so that the terminal's
  Ctrl-C doesn't reach it (it ignores SIGHUP too), with its output in `<state>/daemon.log`
  (moved to `daemon.log.1` past 8 MiB), and returns once the socket answers, or with the log's
  last lines if the daemon exits. Run while one is running, it says so, with status 0, as
  Space's. `daemon run` refuses at once when a daemon answers.
- **`daemon stop` waits** until the daemon has stopped and let go of the state directory, at most
  a minute, so that `restart` and scripts can count on it; Space's returns at once and leaves
  its socket behind. What is uploading stops at its next request, and goes on from the journal
  when the daemon starts again. SIGTERM and SIGINT stop it the same way.
- **Restart safe** (`daemon info`) is yes while no change is held only in memory. The journal
  syncs every change before its call returns, so it is yes today; `memoryOnlyBytes` is where
  step 5's mount will count writes it holds before journaling them. `info` also says what a
  restart costs: the entries uploading now are sent again (a large file from its last finished
  part), and imports are read from where they are, so they must stay there. Space counts
  write-back bytes, dirty handles, requests in flight and resumable imports (§1.2). Unlike
  Space's, `info` has `--json`, and with no daemon it fails (`DaemonNotRunning`, status 1),
  where Space's prints "daemon not running" with status 0.
- **`status`:** the daemon, the server and the link to it (online, degraded or offline), the
  upload queue counted, and the cache. With no daemon it is `{"daemon": {"running": false,
  "socket": …}}`, as Space's. The mounts and their feeds join it with the mount table.
- The daemon's log is lines with times on its stderr: its start, its stop and why. A stderr that
  went away (the terminal that ran `void daemon run`, closed) doesn't stop it: a write that fails
  is dropped. Before, its next line panicked, and the daemon died leaving its socket; `daemon
  stop` also takes a state directory nothing holds as stopped, socket or not.

**Design.** The `void` binary also runs as the per-user agent (`void daemon run`), as
`spacefs-fskitd` is both; the spike decided on a per-user agent for the Rust core (spike §4.1).
- A Unix socket in the state directory, HTTP with JSON over it (the Mac app will speak the same).
  Requests: status, uploads (list, watch, pause, resume, cancel, limit), upload (enqueue a batch),
  mounts (the table and the remembered mounts), info (build, and the journal's unpublished bytes).
- `void daemon start|stop|restart|status|info|install|uninstall`: `install` writes a launchd
  agent on macOS (a systemd user unit in step 9), so that the daemon and remembered mounts come
  back at login.
- `void upload … [--detach]` enqueues through the daemon; `void uploads [--watch] [--json]
  [pause|resume|cancel|limit]`; `void status [--json]`: daemon, mounts, uploads, the feed, the
  connection, on one page. Unlike SpaceFS's, it sees every mount the daemon serves, whatever the
  adapter (SpaceFS's misses its own SMB mount, §1.2).
- `void mount|unmount|mounts`: the table and the remembered mounts land here; mounting itself
  comes with step 5's adapter.

**Checked by:** the daemon started in tests on a socket of its own, with the CLI against it;
uploads surviving a daemon restart.

### Item 5. Direct uploads

**Checklist:** E10 (protocol §4.11, as written).

**Design.**
- **Server.** Plan: up to 4,096 shards; which the pool holds safely (§12.4 of the format: present,
  and not proposed for deletion, or re-touched); presigned PUT URLs for the rest that bind the
  shard's SHA-256 (`x-amz-checksum-sha256`) and, where the bucket supports it, `If-None-Match: *`;
  a stateless token, an HMAC over the drive, the key, the list's digest and the expiry (900 s).
  Commit: check the token and the list, HEAD each shard the plan listed as missing for its length
  (the bound checksum vouches for its bytes), then commit a put of those extents through the
  ordinary path, with its preconditions. A store that can't presign (memory, local disk, or a
  bucket the probe finds lacks checksum binding) answers `501`, and clients fall back. The probe
  reports whether presigned PUTs bind checksums and `If-None-Match`.
- **Client.** `put_object_direct` in the SDK (opt-in, bodies of 8 MiB and up, as SpaceFS's),
  falling back to a put on anything but `409` and `412`; the upload queue uses it for files whose
  shards a drive mostly holds (a file re-uploaded after a small change), and an ordinary put for
  new content, which SpaceFS also says is faster.

**Checked by:** new conformance cases (held counts, refused commits for a missing shard, a wrong
or expired token, preconditions, the fallback); a run on versitygw, AWS S3 and R2 that a shard
with the wrong bytes is refused by the bucket; a measurement of a 32 MiB re-upload with one changed
shard (bytes sent and time, against a put).

### Item 6. Short-lived storage credentials

**Checklist:** B4 (protocol §5.5, as written).

**Design.**
- **Server.** Read-only credentials for `shards/`, `pages/` and `drives/<id>/` under the pool's
  root: on AWS through STS AssumeRole with a session policy (a role the deployment names); on R2
  through Cloudflare's temporary-credentials API (an account id and API token); on MinIO through
  its STS. `accessGeneration` from the drive's access rules; `storageBudget` once quotas exist
  (S9). Anything else keeps answering `501`.
- **Client.** A fetcher that reads shards and the drive's metadata straight from the bucket,
  renewing the credentials before they expire, and keys the cache on `accessGeneration`. Reading
  metadata from the bucket needs a read-only reader of the format (the drive's log, checkpoints
  and pages) in the client: the server's loader, factored out of `pool.rs` into a shared crate.

**Decisions this item needs from the user (not taken here):**
- §5.5 gives credentials only. Presigned GET URLs per shard, the plan's fallback "where the
  backend can't mint credentials", and a call that returns an object's shard list (so that a
  client reading through the API can cache shards by hash too), would each be a protocol
  addition, so an RFC first.
- Which providers to support first. AWS and R2 need credentials the deployment holds (a role, an
  API token): B5's stored credentials.

**Checked by:** the credentials refused for writes and for other prefixes, on AWS and R2 (asked
for first); a cold read through the bucket against one through the API.

## 5. Out of scope for step 4

- Accounts: `login`, `logout`, `whoami`, workspaces, minting and revoking keys (step 6).
- The mount adapters (FSKit, SMB, FUSE), Finder integration, and the menu-bar app's transfer
  view (step 5); the Linux packaging and the systemd unit (step 9). Step 5 should weigh the
  loopback SMB server SpaceFS 0.2.333 now mounts with by default (§1.3) against FSKit.
- Self-update (`update`, D10), Windows, encryption, pinning's UI (D8, though the cache supports
  pins from item 3).
- The TypeScript, Python and Go SDKs (step 7).

## 6. Status

- Item 1, the Rust SDK: **done** (1 October). Its 35 tests (14 unit, 11 against a server in the
  same process, 10 through the fault proxy) each failed with the code it guards broken, 37 breaks
  in all, each run alone with a timeout. The conformance suite passes through the moved signing
  code on memory, local disk and versitygw, both addressing styles.
- Item 2, the CLI (`void`): **done** (1 October). Its 29 tests (12 unit, and 17 that run the
  binary against a server in the same process, every command in text and JSON and its errors)
  each failed with the code it guards broken, 63 breaks in all, each run alone with a timeout;
  two of them break the core fix and the SDK change it brought. The conformance suite passes on
  memory, local disk and versitygw, and the README's commands were run by hand. The two server
  issues it found are fixed (item 2, above).
- Item 3, the client core: **done** (2 October). The cache and fetcher are built: their 13
  tests (9 unit, 4 against a server in the same process and through the fault proxy) each failed
  with the code they guard broken, measured as
  [bench/results/client-cache](../bench/results/client-cache/README.md) says. The write journal and
  the upload queue are built: their 13 tests (12 against a server in the same process and through
  the fault proxy, 1 unit), and the SDK's 3 for its bandwidth limit and multipart calls, each
  failed with the code they guard broken, 35 breaks in all. The change-feed client and
  connectivity are built: their 7 tests (3 unit, 4 against a server in the same process and
  through the fault proxy), and the SDK's 1 for its observer, each failed with the code they guard
  broken, 16 breaks in all.
- RFC 0004, which the user accepted on 2 October, is implemented: folder restores are in every
  restored file's history in a pool with `multi-object-versions` (item 2, above).
- Item 4, the daemon and the CLI's daemon commands: **in progress** (3 October). The daemon and
  its socket are built, with `void daemon run|start|stop|restart|status|info` and `void status`:
  their 12 tests (2 unit, 3 with the daemon in the test's own process, and 7 that run the binary
  against a server in the same process) each failed with the code they guard broken, 27 breaks
  in all, each run alone with a timeout. Space's daemon was observed running (§1.2). Next:
  uploads through the daemon, then the mount table and the launchd agent.
- Items 5–6: not started.
