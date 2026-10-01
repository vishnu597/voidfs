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

### 1.2 The CLI (read: `spacefs --help` and every subcommand's, 0.2.333; observed: the read-only commands)

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
2. **The CLI on the protocol** (D9): `drives`, `drive`, `fork`, `history`, `show`, `restore`,
   `version`, and a foreground `upload`, with `--json` everywhere.
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

**Design.** A crate `voidfs-cli` with the binary `voidfs`, on the SDK:
- `voidfs drives [--json]`: alias, id, created, size (describe for each, a few at a time).
- `voidfs drive create <name> [--display-name] [--json]`, `drive delete <name> [--hard] [-y]`
  (asks for the name unless `-y`, as SpaceFS's does), `drive undelete <name>`, `drive show
  <name> [--json]`. SpaceFS's `drive rename` renames a display name, which voidfs doesn't store
  yet (PARITY §3, S3), so it waits for that.
- `voidfs fork <source> <name> [--json]`: SpaceFS has no command for it, but forks need no
  account, and agents use them.
- `voidfs history <drive> <path> [--all] [--json]`, `voidfs show <drive> <path> [--at <time> |
  --version <id>] [-o file]` (to stdout by default, streamed), `voidfs restore <drive> <path>
  [--at <time> | --version <id>] [--json]`. `--at` takes RFC 3339 or Unix seconds, as SpaceFS's
  does. A drive is named by alias or id everywhere, where SpaceFS's three take only the id.
- `voidfs upload <paths…> <drive>:[/folder]`: in this item it runs in the foreground (multipart
  through `client.s3()` above 64 MiB), with progress on stderr and a JSON summary with `--json`;
  item 4 hands it to the daemon.
- `voidfs version [--json]`; `voidfs keys generate [--scope read|write|admin] [--format
  text|env|json]`, which makes a key for `voidfs-server --key`: the self-hosted counterpart of
  `keys create`, which needs no account.
- Configuration from the SDK's environment variables, or `--endpoint` and a key file. `--json`
  prints one JSON document on stdout and errors as JSON on stderr, with the exit status set.

**Out of scope:** `login`, `logout`, `whoami`, `workspace`, `use`, `keys list|revoke` (accounts,
step 6); `update` (self-update, D10).

**Checked by:** the binary run against the in-process server in integration tests, for each
command in text and JSON, and the README's commands run by hand.

### Item 3. The client core

**Checklist:** D1 (with D6's cache and D8's pins started).

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
cancel, the cap's rate within 10%, and resumption after restart; feed reconnection and the `410`
relist.

### Item 4. The daemon and the CLI's daemon commands

**Checklist:** D1, D9.

**Design.** The `voidfs` binary also runs as the per-user agent (`voidfs daemon run`), as
`spacefs-fskitd` is both; the spike decided on a per-user agent for the Rust core (spike §4.1).
- A Unix socket in the state directory, HTTP with JSON over it (the Mac app will speak the same).
  Requests: status, uploads (list, watch, pause, resume, cancel, limit), upload (enqueue a batch),
  mounts (the table and the remembered mounts), info (build, and the journal's unpublished bytes).
- `voidfs daemon start|stop|restart|status|info|install|uninstall`: `install` writes a launchd
  agent on macOS (a systemd user unit in step 9), so that the daemon and remembered mounts come
  back at login.
- `voidfs upload … [--detach]` enqueues through the daemon; `voidfs uploads [--watch] [--json]
  [pause|resume|cancel|limit]`; `voidfs status [--json]`: daemon, mounts, uploads, the feed, the
  connection, on one page. Unlike SpaceFS's, it sees every mount the daemon serves, whatever the
  adapter (SpaceFS's misses its own SMB mount, §1.2).
- `voidfs mount|unmount|mounts`: the table and the remembered mounts land here; mounting itself
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
- Items 2–6: not started. Next: the CLI (item 2).
