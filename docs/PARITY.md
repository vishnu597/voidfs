# voidfs and SpaceFS: parity status and plan

*Stocktake of 2026-09-28, brought up to date the same day after content-defined checkpoints, the
capability probe, group commit and the shard cache's admission, on 2026-09-29 after
virtual-host addressing and then health checks and metrics, and on 2026-10-01 for cold reads and
S3's bandwidth in the local benchmark, then coalesced shard fetches, and then for the start of
step 4 (the scorecard refreshed, SpaceFS 0.2.333 looked at again, the Rust SDK, then the CLI),
and then for the Mac apps head to head; provider follow-ups and the step 5 plan added on
2026-10-04, with step 5 decisions, namespace and snapshot-read slices on 2026-10-05, and bounded
moved-snapshot resolution, local namespace changes and staged file writes on 2026-10-06,
Rust daemon sessions/shared feeds and guarded publication on 2026-10-07, and mount recovery on
2026-10-08. The first was taken
on 2026-09-27.*

Sources:
- the voidfs code on `origin/main` at `4b4d62d` (Rust daemon sessions merged),
  plus this change's stable remembered identities, local notifications and socket ownership fixes;
- the parity checklist in [§3 of the plan](RESEARCH_AND_PLAN.md#3-parity-checklist-everything-to-build);
- the benchmark results in [`bench/results/`](../bench/results/);
- SpaceFS's benchmark pages (runs of 20 and 23 September 2026) and changelog, read again on 28
  September: no release since 22 September, and the same benchmark runs;
- the SpaceFS macOS app 0.2.300, installed on the same Mac, which had updated itself to 0.2.333
  by 1 October: its CLI's help, its read-only commands, and the app's settings looked at through
  computer use ([step 4's plan, §1](step-4-client.md#1-what-spacefs-offers)); and SpaceFS's
  `llms-full.txt`, read on 1 October.

The goal is feature, functionality and performance parity with SpaceFS, as open source that people
host themselves on their own bucket. Where voidfs differs on purpose (no hosted offering, so no
billing or plans), this page says so.

## 1. Summary

- **The foundation is built.** The storage engine and the S3 layer are the parts that are hardest
  to get right, and they are largely there:
  - versioned, content-chunked storage;
  - in-place edits and O(1) renames;
  - forks, point-in-time reads and restore;
  - a change feed;
  - garbage collection, whose protocol is model-checked
    ([RFC 0002](../rfcs/0002-gc-safe-against-writers.md));
  - 38 conformance cases passing on local disk (the first 35 on Cloudflare R2 too).
- **Almost everything a person touches is missing:** a writable mount, accounts and a web app,
  search and share links. The first SDK, for Rust, exists (step 4, item 1): the official AWS SDK
  for S3 with a typed call for every extension, one error type and the retry rule. So does a CLI
  on the protocol, `void` (step 4, item 2): drives, forks, history, show, restore and a foreground
  upload, with JSON output everywhere. The client core exists (step 4, items 3–6): a block
  cache on the Mac with read-ahead, which turns the head-to-head's random reads from 140–164 ms
  into hits (p50 0.06–0.15 ms, R2 included, as Space's 1.2 ms are) and streams a reader that waits
  for each read (37–40 MB/s from R2, as fast as Space's Finder copy); a write journal and upload
  queue that pause, resume and cancel anything, resume after a restart, and cap bandwidth at once;
  and a change-feed client and a connectivity state that fails fast offline. A per-user daemon
  runs it, with uploads that outlive it; a file changed in one place goes up as a direct upload;
  and reads come straight from the bucket where the server issues storage credentials.
- **Performance is measured locally, not yet in SpaceFS's setup.** SpaceFS publishes 49
  benchmark scenarios; [`bench/`](../bench/README.md) runs all of them against voidfs and the bare
  bucket underneath it.
  - In the closest local emulation (the bucket 12 ms away), voidfs is at least as far ahead of
    the bare bucket as SpaceFS on 25–30 of the 49 rows (§6): 29 in both runs of the latest change,
    25–27 for `main` in the same session (the full runs move by a few rows; focused runs settle the
    rows a change targets), up from 16–19 with group commit alone and 7 before it. Its geometric
    mean speed-up over the bare bucket there, 3.0×, is above SpaceFS's 2.8×.
  - Group commit removed the cause that held back 35 rows: a drive committed one mutation per
    bucket round trip. The shard cache now keeps what was written and read last, which put the
    fan-out gets far ahead. Large puts now upload while they read the body, and checkpoints no
    longer hold up commits: put 64 MiB takes 1.3× the bare bucket's time eight at once and 0.9×
    alone, from 2.2× and 2.5×. Small files are now held in the log (RFC 0003), and each log entry
    waits briefly for the requests the one before it answered: a small write or a rename takes
    one round trip instead of two, alone or 64 at once. A patch now chunks each shard it touches
    once, not once per edit: patch in 1 and 32 MiB crossed SpaceFS's ratio. Completing a
    multipart upload takes two round trips instead of a chain of them. Then patch in 64 MiB, and
    large reads that only the real run can judge.
  - **With S3's bandwidth emulated as well** (1 October: the bucket 12 ms away and capped as S3
    was in SpaceFS's run, a different setup from the figures above), 45–46 of the 49 rows are at or
    ahead of SpaceFS's ratio in both runs of each build, and the geometric mean speed-up is 5.8–5.9×.
    Behind are warm reads of 64 and 256 MiB, at 0.78–0.94 of SpaceFS's ratio, which this Mac's
    loopback limits, and in one run the range read, which sits on SpaceFS's ratio.
  - **Cold reads are now measured** (the server drops its caches on `SIGUSR1`), and **ahead of
    SpaceFS's cache-cleared figures** with the cap: since concurrent fetches of a shard are
    coalesced and a read borrows read-ahead from a shared budget (1 October), get 32 and 64 MiB
    take 0.18–0.31× the bare bucket's time, where SpaceFS's took 0.38–0.50× and `main`'s
    0.71–0.76×. The small cold reads were ahead already.
- **SpaceFS's Mac app is now understood** (§5). It is a native FSKit module with its core in Rust,
  running in a separate daemon, which is the architecture the FSKit spike chose for voidfs. It
  also shows that the FSKit entitlement can ship with Developer ID. **Since 0.2.333** (observed 1
  October) it mounts drives through a loopback SMB server by default, backed by a local journal,
  with FSKit as the other choice, and offers pinned files for offline use: step 5 weighs both.
- **The plan (§7):**
  - Step 1 has its harness, local results and CI, and the Mac apps have been compared on one
    Mac (1 October, §6). Still to do: the run in SpaceFS's setup (which waits on cloud accounts,
    §8).
  - Step 2 is done: garbage collection, content-defined checkpoint segments, the bucket
    capability probe, runs on AWS S3, MinIO and rclone, virtual-host addressing, and the
    Compose file with health checks and metrics.
  - Step 3 has its first six items done: group commit (with a hold, so that concurrent writes
    share one round trip), the shard cache's admission, fewer round trips per write
    (checkpoints in the background, pipelined ingest, and small files held in the log, RFC 0003),
    patch chunking each touched shard once, multipart completion in two round trips, and
    coalesced shard fetches with shared read-ahead. Reading pieces of shards is deferred to the
    mount (step 5), as SpaceFS's S3 layer also reads whole shards for a range.
  - Step 4 (client core, CLI and Rust SDK) is done, items 1–6 of 6 (4 October): the Rust SDK, the
    CLI (`void`), the client core, the daemon, direct uploads and storage credentials
    ([step-4-client.md](step-4-client.md)). R2's live scoped-read check passes all four applicable
    conformance cases. AWS also passes all four after correcting the read role's bucket ARNs;
    required reads return 200 and forbidden operations return 403. The
    [AWS guide](aws-storage-credentials.md) records the initial failure and successful rerun.
  - Step 5 has begun with the [mount core](step-5-macos.md#staged-file-data-6-october):
    persistent inodes, lookup, attributes and directory listings, plus read-only snapshot
    handles through the shared cache with offline reads and restart detection. Moved reads use
    the namespace first and a bounded fallback scan. Durable local namespace/xattr edits and
    staged file writes now share that core. Local edits are visible before upload, including to
    earlier handles; open files queue edits after two seconds without writes. FSKit first,
    the Swift XPC bridge and verified whole shards are accepted; item 1 is complete.
    Rust session RPCs share one core/feed per stable drive across mounts and observers. Remembered
    mounts pin stable IDs; local edits notify peers offline, and sessions authenticate Unix peers.
    Guarded publication reconciles exact acknowledgements and retains local/remote conflicts.
    [Recovery](step-5-macos.md#recovery-8-october) survives process kills at each step, knows a
    lost put reply as its own, compacts staging and removes what nothing needs. The core
    [advertises](step-5-macos.md#capabilities-8-october) what it doesn't support (hard links,
    exchange, cloning, cross-machine locks), refused with `ENOTSUP`, through the daemon's session
    reply too. With [mode and mtime changes](step-5-macos.md#setting-mode-and-mtime-8-october),
    item 1, the mount core, is complete (8 October): step 5's first completed item. A mount
    edit, rename or attribute change whose reply was lost is [known as its own](step-5-macos.md#lost-replies-to-edits-renames-and-attribute-changes-8-october)
    instead of becoming a false conflict, edits by a marker that
    [RFC 0005](../rfcs/0005-user-metadata-on-edits.md) lets them carry; SpaceFS's documentation
    leaves that to the caller. The daemon's socket now carries
    [every mount-core call](step-5-macos.md#the-rest-of-the-session-calls-9-october) (9 October):
    namespace changes, attributes, xattrs and conflict reads. The app's launch agent is now
    [the Swift bridge](step-5-macos.md#the-swift-bridge-9-october): the signed, sandboxed
    extension reaches the daemon through it, with a metadata memo, restart outcomes and hop costs
    recorded. macOS keeps the CLI's daemon out of the app's App Group container, so the app
    shares [the CLI's daemon and store](step-5-macos.md#one-daemon-for-the-cli-and-the-app-9-october)
    where they already are; item 2 is complete once the user accepts that in place of the planned
    move. The writable mount remains pending. Steps 6–10 have not started.

## 2. Decisions that shape the plan

| Decision | Choice | Notes |
|---|---|---|
| Platform order | **Full Mac parity first** (steps 1–8), then Linux (step 9) | Full Mac parity includes search, previews and video review. Linux servers use S3 and the SDKs until the Linux mount lands |
| Minimum macOS | **macOS 27** (decided 2026-09-27) | SpaceFS states macOS 26.4 as its minimum; its app bundles declare 26.0. voidfs gives up macOS 26 because only macOS 27 lets a module evict the kernel's caches when another machine changes a file ([spike §5](spikes/fskit.md#5-kernel-caching-and-coherence)) |
| Mac architecture | Rust daemon and socket, thin Swift XPC bridge, thin FSKit extension | Accepted 2026-10-05. FSKit first; evaluate SMB if its compatibility gate fails. [Step 5 decisions](step-5-macos.md#1-starting-point-and-decisions) |
| Hosting | Self-hosted only | No billing, plans or trials to match. The hosted edge network is matched by deploying near users (step 10) |

## 3. Scorecard

Every item of the plan's checklist (§3, 65 items, including the Finder integration item added on
2026-09-27), scored against the code on `main` at `939b57c` and the results in `bench/results/`
(refreshed 2026-10-01; the previous scorecard was of 28 September):

| Area | Done | Partly | Missing | State |
|---|---|---|---|---|
| Engine (E1–E13) | 10 | 1 | 2 | Chunking, versions, point-in-time reads, restore, in-place edits, forks, checkpoints, garbage collection, small files held in the log (RFC 0003, the `inline-data` pool feature), a folder restore that is one version in the history of every file it changes (RFC 0004, the `multi-object-versions` pool feature), and direct uploads (step 4, item 5: where the bucket binds a shard's checksum to its URL, from the SDK and the upload queue) work. Missing: encryption, retention policies |
| Storage backends (B1–B8) | 2 | 1 | 5 | Local disk, AWS S3, Cloudflare R2, MinIO (built from source in CI) and versitygw work, and rclone works against the server. A capability probe checks the bucket at start. Storage credentials pass scoped-read checks on MinIO, live R2 and live AWS after correcting the read role's bucket ARNs. No other providers tried, no stored bucket credentials, no adopt or export |
| Server (S1–S9) | 3 | 3 | 3 | Full S3 subset, path and virtual-host addressing, extensions and change feed (long poll and SSE), direct uploads where the bucket binds checksums, and storage credentials where STS or Cloudflare's API passes the scoped-read check, on one node. Missing: a disk cache tier, several nodes, several regions, quotas |
| Accounts and web (C1–C10) | 0 | 1 | 9 | Static keys from command-line flags only |
| Clients (D1–D12) | 0 | 5 | 7 | A read-only macOS mount and its menu-bar shell (the spike), the CLI on the protocol, `void` (step 4, item 2), the client core (item 3): block cache, write journal and upload queue, change-feed client, and the daemon that runs it, with uploads handed to it and the mount table (item 4), direct uploads (item 5), and reads straight from the bucket with storage credentials (item 6). No mounting yet (step 5's adapters), no Finder integration, Linux or Windows |
| SDKs, agents, search (A1–A7) | 0 | 1 | 6 | A Rust SDK on the official AWS SDK, with a typed call for every extension (step 4, item 1) and direct uploads (item 5). Stock S3 SDKs, boto3, rclone and curl work. No TypeScript, Python or Go SDK, MCP server or search |
| Operations (O1–O6) | 1 | 2 | 3 | One binary, with a Compose file. Health checks and Prometheus metrics on a port of their own. A benchmark harness run locally, against R2 and AWS S3 for the small objects, and in CI, not yet in SpaceFS's setup. No tracing, Helm chart, fuzzing or audit |
| **Total** | **16** | **14** | **35** | Of the 28 P0 items: 14 done, 9 partly, 5 missing |

What moved since 28 September: E9 (small files held in the log, RFC 0003) is done, and B1 is done
now that AWS S3 and MinIO have run the conformance suite and the clients. A1 is partly done since
1 October, with the Rust SDK (step 4, item 1), and so is D9, with the CLI (step 4, item 2). D1
is partly done since 2 October, with the client core's cache and fetcher (item 3), which a daemon
runs since 3 October (item 4), with background uploads and the mount table. E10 is done since 4
October: the server offers direct uploads, and the SDK and the upload queue use them (item 5).
B4 is partly done since 4 October: the server mints storage credentials scoped to a drive where
the provider can, and the client core reads from the bucket with them (item 6). R2 minting is now
verified live: four applicable conformance cases pass. AWS also passes all four after correcting
the assumed role's bucket ARNs, which initially named a different bucket. This provider
verification does not change the scorecard counts.

"Partly" means:
- E1 has no compression (`shard-zstd` is a reserved pool feature that servers refuse).
- S3 serves storage credentials (§5.5) only where a provider's scoped mint passes the startup
  checks (MinIO STS; AWS STS with a role; R2's Cloudflare API with an account-level token).
  Live AWS and R2 pass those checks; a failed capability check answers `501`.
  The server also lacks copy-mode forks
  (`x-voidfs-fork-mode: copy` answers `501`) and display names (`x-voidfs-display-name` is
  ignored: a drive's display name is its alias). S5 is memory-only. S8 has the protocol header
  and 55 conformance cases, but no operations catalogue.
- C3 keys come from command-line flags: they can't be minted or revoked while the server runs, and
  the drive allowlist exists in the code but no flag sets it.
- D1 has the client core (step 4, item 3): the block cache with read-ahead, the write journal and
  upload queue, and the change-feed client with connectivity; and the per-user daemon that runs
  it on a socket, with uploads handed to it that outlive it, the mount table and the remembered
  mounts it brings back when it starts, at login through a launchd agent (item 4, done on 3
  October). Not yet mounting itself (step 5's adapters).
- D3 is read-only, D6 relies on the kernel's read-ahead only (the client core's cache and
  read-ahead reach the mount in step 5), and D11 is the spike's shell.
- D9 has the commands that need no account (`void`, step 4, item 2), and since 3 October `void
  daemon run|start|stop|restart|status|info|install|uninstall`, `void status`, `void upload
  --detach`, `void uploads` with pause, resume, cancel and a limit, and `void
  mount|unmount|mounts` (item 4), which mount once step 5's adapters exist: not yet login,
  workspaces, minting keys or self-update.
- A1 has the Rust SDK; TypeScript (P0) waits for step 7, with Python and Go (P1).
- O4 has health checks and metrics, not tracing or structured logs. O5 has run locally, against R2
  and AWS S3 (the 23 small-object scenarios) and in CI, not yet in SpaceFS's setup.

Two items count as done with a gap that can't matter yet:
- E7: a fork across pools answers `501` rather than copying, but a deployment has one pool.

The 28 P0 items, which a credible v1 needs:

| Status | Items |
|---|---|
| Done (14) | E2 format spec, E3 namespace, E4 versions, E5 edits, E6 commit protocol, E7 forks, E8 garbage collection, E11 checkpoints, B1 backends, B3 capability probe, S1 S3 server, S2 S3 subset, S4 per-drive authority and change feed, O1 single binary and compose |
| Partly (9) | E1 chunking, S3 extensions, S5 shard cache, S8 conformance and catalogue, C3 access keys, D1 client daemon (the core, and the daemon that runs it), D3 macOS mount, D9 CLI (the commands on the protocol), A1 Rust and TypeScript SDKs (Rust done) |
| Missing (5) | B5 stored bucket credentials, C1 sign-in, C2 workspaces, C4 bucket connections, D5 desktop semantics |

## 4. Product by product

| SpaceFS | voidfs today |
|---|---|
| S3-compatible endpoint with extensions (protocol v1) | Protocol 1 with 55 conformance cases, path or virtual-host addressing, direct uploads where the bucket binds checksums (AWS S3, R2, MinIO and versitygw do), and storage credentials through provider minting with a startup scope check. MinIO, live R2 and live AWS pass. Missing: display names, copy-mode forks |
| macOS app: writable mount (a loopback SMB server by default since 0.2.333, or FSKit), transfers (pause, resume, speed limits), a disk and a memory cache, pinned files, previews, video reviews, Space Search, Finder badges | Read-only FSKit mount and a menu-bar shell |
| `spacefs` CLI and mount daemon (macOS and Linux): login, whoami, drives, drive, workspace, use, keys, mount, unmount, mounts, uploads, upload, status, daemon, history, show, restore, version, and `update` | `void`: drives, drive create/show/delete/undelete, fork, history, show, restore, upload (foreground or daemon-backed with `--detach`), uploads with pause/resume/cancel and a limit, status, daemon run/start/stop/restart/status/info/install/uninstall, and mount/unmount/mounts with remembered mounts. JSON output everywhere. Mounting itself waits for step 5's adapters; account commands wait for step 6 ([step-4-client.md](step-4-client.md)). |
| Linux FUSE mount | None (step 9) |
| Web app: sign-in, settings, access keys, billing | None (billing does not apply) |
| SDKs: TypeScript, Python, Go, Rust (MIT), each on the official AWS SDK | Rust (`crates/voidfs-sdk`): the official AWS SDK for S3, a typed call for every extension, one error type, the retry rule, and direct uploads, opt-in as SpaceFS's are (step 4, item 5). Not yet: TypeScript, Python, Go |
| Agent docs: `llms.txt`, `llms-full.txt`, `operations.json` | None |
| Public and restricted share links | None |
| Passwordless email sign-in, workspaces, scoped keys with drive allowlists, revocation within a minute | Static scoped keys; the allowlist exists in the code but cannot be set |
| Enterprise: SSO and SAML, audit, retention controls, private cloud, bring your own storage | Bring your own storage is the default. The rest is missing |
| Windows: "coming soon" | Neither has it |

## 5. What SpaceFS's Mac app is made of

Read from the installed app (`/Applications/Space/Space.app`, version 0.2.300) on 2026-09-27:
bundle metadata, entitlements and linked libraries only.

| Part | What it is |
|---|---|
| `Space` | The SwiftUI menu-bar app (`com.spacefs.launcher`). It links FSKit, ServiceManagement, SQLite, QuickLook thumbnailing, AVKit and Vision |
| `Extensions/SpaceFSExtension.appex` | **An FSKit module** (`com.apple.fskit.fsmodule`, short name `SpaceFS`). It uses `FSPathURLResource` with security-scoped URLs, not a generic URL. It is sandboxed with `network.client` and the App Group `group.com.spacefs.launcher` |
| `MacOS/spacefs-fskitd` (and `spacefs`, a symlink to it) | **A Rust binary** (tokio, hyper, `aws-sdk-s3`) that is both the CLI and the mount daemon |
| `MacOS/SpaceMountHelper` + `Library/LaunchDaemons/…MountHelper.plist` | A privileged LaunchDaemon (registered through ServiceManagement) with an XPC service. It has no FSKit mount entitlement, so it presumably mounts through `mount(8)` as root, into `/Volumes` |
| `PlugIns/SpaceFinderSync.appex` | A Finder Sync extension: badges and context menus in Finder |
| `Resources/space-launcher-search` | A separate search process, reached through a Unix socket |
| Signing | Developer ID, notarized, arm64 only. The extension carries a **Developer ID provisioning profile** ("Spacebar FSExt DevID-Dist", all devices) that grants `com.apple.developer.fskit.fsmodule` |

What voidfs takes from this:
- Distributing an FSKit module with Developer ID works, which removes the spike's main
  distribution risk.
- `/Volumes` does not need the `fskit.mount` entitlement: a privileged helper can do the mount.
- Finder Sync, not File Provider, gives the Finder badges. It is now checklist item D12.
- The split between a thin extension and a Rust core in a separate process is confirmed by a
  shipping product.

**What changed by 0.2.333** (observed on 1 October, when the app had updated itself; details in
[step 4's plan, §1](step-4-client.md#1-what-spacefs-offers)):
- The drive is mounted by `spacefs mount --retained-v1 --adapter smb`, a loopback SMB server on
  this Mac, at `/tmp/Space`; Settings → Advanced → Mount Method says SMB, "backed by a local
  journal", and the FSKit daemon is "Not running". FSKit stays a choice.
- The journal is one SQLite database per drive: inodes, written blocks, fetched blocks, pins and
  the publishing state (from its schema; no rows were read).
- Settings show an upload bandwidth slider (adaptive by default, a cap, or unlimited), a 20 GB disk
  cache and a 192 MB memory cache, and **pinned files** for offline access, which voidfs's plan had
  after parity (D8).
- New processes, `space-agent` and `space-computer-use`, belong to its AI features (inferred).
- Its CLI's `status` and `mounts` report no daemon and no mounts while the SMB mount is up.

**Seen on 3 October** (0.2.343, the trial's last day; [step 4 §1.5–§1.9](step-4-client.md#15-direct-uploads-observed-through-the-api-3-october-02343-protocol-1)):
- The S3 API runs at Cloudflare's edge (Workers and Durable Objects); objects' bytes are in Backblaze
  B2 and their metadata in R2, in 16 storage groups per drive, each owned by a worker under a lease;
  the mount works through an engine on Railway and doesn't show objects written through the API.
- Direct uploads presign URLs that sign only the host; the commit checks each new shard instead.
- A cold random read through the mount fetches a whole 8 MiB block, over SMB and over FSKit; over
  SMB, file modes and resource forks are lost, over FSKit they are kept.
- Keys revoke at once (0.3 s, children too); pins download the whole file.

## 6. Performance

### What SpaceFS publishes

**Near the bucket** (20 September 2026, run `20260920T055107Z`):
- Setup: client on a Google Cloud n2-standard-8 in us-east4; bucket on AWS S3 us-east-1 with
  native conditional writes.
- Method: Rust `aws-sdk-s3`, 8 operations at a time, 3 warm-ups, 2 rounds, median of each.
- Every scenario runs once through SpaceFS and once against the bare bucket.
- SpaceFS wins 31 of 49 scenarios.

| Scenarios | Rows | SpaceFS against the bare bucket | Example: SpaceFS / bare, p50 |
|---|---|---|---|
| Small, ranged and cached reads, `head`, fan-out gets | 7 | 2.6–34× faster | 64 KiB range of a 64 MiB file: 1.3 / 44.1 ms |
| Large gets and streams (32–256 MiB) | 4 | 12–16× faster | get 64 MiB: 47.2 / 779 ms |
| Edits inside 32 and 64 MiB files (append, insert, delete, truncate, write, patch) | 16 | 1.4–15× faster | append 4 KiB to 64 MiB: 111 / 1,668 ms |
| Rename and folder move | 2 | 7.9–18× faster | move a folder of 200 files: 99 / 1,774 ms |
| Listing | 1 | 9.1× faster | 200 keys: 3.0 / 27.2 ms |
| Edits inside 1 MiB files | 8 | parity to 2.1× slower | write 4 KiB in 1 MiB: 157 / 101 ms |
| Whole-object puts and overwrites, fan-out puts | 9 | 1.1–3.1× slower | overwrite 4 KiB: 84.6 / 27.7 ms |
| Multipart uploads | 2 | 1.8–2.4× slower | 64 MiB in 8 MiB parts: 1,613 / 660 ms |

**Around the world** (23 September 2026): the same workload from six regions, as a geometric mean
against S3 us-east-1.

| Region | Own drive | Drive in Virginia |
|---|---|---|
| Sydney | 9.7× | 5.5× |
| Singapore | 5.7× | 4.8× |
| Frankfurt | 2.2× | 1.8× |
| Oregon | 1.4× | 1.4× |
| Virginia | 0.8× | 0.6× |
| São Paulo | 0.8× | 0.8× |

SpaceFS publishes no numbers for its mount.

### Where voidfs stands

*First measurements, 27 September 2026: [bench/README.md](../bench/README.md#results-so-far).*
All 49 scenarios ran on one Mac, against a local S3 server (versitygw), once over loopback and
once with the bucket 12 ms away; 23 of them also ran against Cloudflare R2. **These are not comparable with SpaceFS's cloud figures**; the
real run in their setup is still to do (§8).

| Scenarios | Rows | Loopback | Bucket 12 ms away | 12 ms, group commit (28 Sep) | 12 ms, shard cache (28 Sep) | 12 ms, write round trips (29 Sep) | 12 ms, small files in the log (29 Sep) | 12 ms, group-commit hold (30 Sep) | 12 ms, patch once (30 Sep) | 12 ms, multipart completion (30 Sep) | **Another setup:** 12 ms and S3's bandwidth (1 Oct) | **Another setup:** 12 ms and S3's bandwidth, coalesced fetches (1 Oct) | SpaceFS |
|---|--:|---|---|---|---|---|---|---|---|---|---|---|---|
| Small, ranged and cached reads, `head`, fan-out gets | 7 | 1.1× slower to 4.6× faster | 1.1× slower to 48× faster | 1.1× slower to 51× faster | 12–48× faster | 12–46× faster | 11–48× faster | 13–51× faster | 11–55× faster | 12–55× faster | 12–53× faster | 12–48× faster | 2.6–34× faster |
| Large gets and streams | 4 | 1.1–1.7× slower | 1.1–3.0× faster | 1.1× slower to 3.1× faster | 2.3–3.1× faster | 2.3–2.7× faster | 2.3–2.8× faster | 2.3–2.7× faster | 2.4–2.9× faster | 2.4–2.9× faster | 12–14× faster | 12–14× faster | 12–17× faster |
| Edits inside 32 and 64 MiB files | 16 | parity to 24× faster | 1.1× slower to 3.3× faster | 1.1× slower to 8.2× faster | 1.5–8.3× faster | 1.5–7.8× faster | 1.4–9.7× faster | 1.3–9.1× faster | 1.5–8.0× faster | 1.6–7.9× faster | 3.4–37× faster | 3.3–38× faster | 1.4–15× faster |
| Rename and folder move | 2 | 44–138× faster | 1.5–4.8× faster | 5.8–18× faster | 5.2–18× faster | 5.0–18× faster | 5.7–20× faster | 11–31× faster | 9.9–29× faster | 9.3–30× faster | 9.5–28× faster | 9.4–31× faster | 7.9–18× faster |
| Listing | 1 | 31× faster | 34× faster | 37× faster | 36× faster | 31× faster | 33× faster | 35× faster | 37× faster | 39× faster | 42× faster | 41× faster | 9.1× faster |
| Edits inside 1 MiB files | 8 | 1.6–4.7× slower | 3.4–3.6× slower | 1.3–1.7× slower | 1.3–1.7× slower | 1.3–1.7× slower | 1.3–1.6× slower | 1.3–1.5× slower | 1.3–1.4× slower | 1.3–1.4× slower | 1.1–1.2× faster | 1.1–1.2× faster | 2.1× slower to parity |
| Whole-object puts and overwrites, fan-out puts | 9 | 1.7–4.5× slower | 2.1–70× slower | 2.1–3.1× slower | 2.2–3.0× slower | 1.3–3.1× slower | 1.3–2.8× slower | 1.1–2.9× slower | 1.1–2.9× slower | 1.1–2.9× slower | 2.1× slower to 1.8× faster | 2.2× slower to 1.8× faster | 1.1–3.1× slower |
| Multipart uploads | 2 | parity to 1.2× faster | 1.6–1.9× slower | 1.6–1.9× slower | 1.5–1.8× slower | 1.2–1.7× slower | 1.2–1.8× slower | 1.4–1.5× slower | 1.4× slower to 1.3× faster | 1.0× slower to 1.0× faster | 1.0× faster | 1.1× slower to 1.0× faster | 1.8–2.4× slower |
| **All 49**: faster in / geometric mean | | 26 / 2.1× | 27 / 1.0× | 24–26 / 2.0× | 30 / 2.6× | 30 / 2.6× | 30 / 2.8× | 30 / 2.9× | 31 / 2.9× | 31 / 3.0× | 42 / 5.9× | 41 / 5.8× | 31 / 2.8× |

The group-commit, shard-cache, write-round-trip, small-file, hold, patch and multipart columns are
the median of each row over two runs ([bench/results/group-commit](../bench/results/group-commit/README.md),
[bench/results/shard-cache](../bench/results/shard-cache/README.md),
[bench/results/write-round-trips](../bench/results/write-round-trips/README.md),
[bench/results/small-content](../bench/results/small-content/README.md),
[bench/results/group-commit-hold](../bench/results/group-commit-hold/README.md),
[bench/results/patch-once](../bench/results/patch-once/README.md),
[bench/results/multipart-complete](../bench/results/multipart-complete/README.md)). The column of
1 October is **another setup**: the same relay, also capped at S3's bandwidth as their run's bare
bucket shows it (`BENCH_BANDWIDTH=s3`), with this branch's server, median of two runs
([bench/results/cold-reads](../bench/results/cold-reads/README.md)). Its rows are not comparable
with the columns before it, only with SpaceFS's. The column after it is the same setup with
coalesced shard fetches and shared read-ahead, median of two runs
([bench/results/shard-fetch](../bench/results/shard-fetch/README.md)): warm, the change does not
show; cold, below, it does.

What the runs show:
- **The prediction held for metadata:** listing, `head` and small warm reads are far ahead of
  the bare bucket at any distance, and further ahead than SpaceFS.
- **Against Cloudflare R2** (23 small-object scenarios, voidfs-server beside the harness on the
  Mac, the bucket about 200 ms per PUT away): reads and metadata ran 15–324× faster than the
  bare bucket, and every write ran at its concurrency times one PUT, up to 13 s. Run again on
  29 September with group commit and the shard cache (a 64 MiB cache, so that it fills): writes
  took 0.6–1.0 s, the fan-out gets 0.6–2.2 ms instead of `main`'s 75–116, and 16 of the 23 rows
  were at or ahead of SpaceFS's ratio against the earlier run's bare bucket
  ([bench/results/shard-cache](../bench/results/shard-cache/README.md#against-cloudflare-r2)).
- **Everything that wrote was held back by one thing:** a drive committed one mutation per
  bucket round trip, because the commit lock was held across the log's conditional PUT. At 8
  operations at once, every write cost 8 round trips; at 64, 64. That is why renames and
  large-file edits lost most of their lead once the bucket was far away, and why fan-out puts were
  up to 70× slower. Group commit (step 3, item 1) fixed it: mutations that wait while a log entry
  is written share the next one, and a write's p50 now stays at two or three round trips from 1
  to 64 at once.
- **The shard cache stopped admitting new shards** once shards read often earlier filled it
  (moka's TinyLFU admission), so warm reads silently went to the bucket. It now keeps what was
  used last (step 3, item 2): the three fan-out gets take 0.5–1.1 ms instead of 12–15, get
  32 MiB 25 ms instead of 56–88, and 12 of the 16 edits inside 32 and 64 MiB files, which read
  the shard they rewrite, are 11–57% faster.
- **A large put took in its body and uploaded it in turns**, and the commit that made a checkpoint
  due waited for it. Ingest now keeps reading while a window of shards uploads, and checkpoints
  are written in the background (step 3, item 3): put 64 MiB at 12 ms takes 210 ms instead of 373
  (1.3× the bare bucket instead of 2.2×), 122 ms one at a time (0.9×), and the writes that land on
  a checkpoint no longer take 120–165 ms. Half of a large put's CPU had been the upload checksum.
- **A small write took two round trips one after the other**: its shard, then the log entry. Small
  files are now held in the log itself (step 3, item 3, [RFC 0003](../rfcs/0003-small-content-in-descriptors.md)):
  alone, put and overwrite 4 KiB at 12 ms take 14.9 ms instead of 29 (1.04× the bare bucket), and
  checkpoints store the files as shards in the background. Eight or more at once still took two
  round trips, for group commit's reason, until each log entry waited for the requests the one
  before it answered (30 September, below): now one.
- **A patch re-chunked and re-hashed a shard for every edit in it**: sixteen edits in a 1 MiB
  file hashed about 16 MiB, 98% of the server's CPU in that scenario. Edits that share a shard are
  now applied to it together and it is chunked once (step 3, item 4, 30 September, below).
- **Completing a multipart upload was a chain of round trips**: the upload's record, a listing,
  each part's record one after another, the commit, and a delete, before it answered. It now reads
  the records together and answers once it commits (step 3, item 5, 30 September, below).
- The mount was measured against SpaceFS's own app on 1 October (below). Before that, the FSKit
  spike's loopback numbers (2.3–2.5 GB/s sequential, 1,000 files listed in 21–33 ms, random 4 KiB
  reads at 1.5–1.9 ms p50) were the only ones for it.

### The Mac apps head to head

*1 October 2026, one Mac on home internet ([bench/results/mac-head-to-head](../bench/results/mac-head-to-head/README.md)).*
SpaceFS's app 0.2.333 with its trial drive, mounted through its loopback SMB server, against the
read-only FSKit spike with `voidfs-server` on the same Mac and the pool in Cloudflare R2 (location
hint ENAM). Space's mount connects to Cloudflare and to Railway; the bytes of objects written
through its S3 API are in Backblaze B2 (Canada East) and their metadata in R2 (observed on 3
October, [step 4 §1.5–§1.8](step-4-client.md#15-direct-uploads-observed-through-the-api-3-october-02343-protocol-1)). The same 11,005 files and
1 GiB file in each, runs alternated, every run cold, two runs each; on-screen steps were driven
through computer use and timed from screen captures.

| | SpaceFS | voidfs |
|---|---|---|
| Mount, from the app | 5.3–7.1 s | 0.07–0.31 s |
| Finder: 1,000 files / 10,000 files, icon view | 0.26–0.47 s / 5.4 s | ≤ 0.15 s / 1.9–2.8 s |
| Finder: copy the 1 GiB file out | 28–30 s | 57–67 s |
| `getattrlistbulk`, 10,000 files, cold | 68–71 ms | 114–137 ms |
| `ls -l`, 10,000 files, cold | 137–142 ms | 1.56–1.60 s |
| `dd` of the 1 GiB file, cold | 38–42 MB/s | 32–36 MB/s |
| Random 4 KiB reads, cold, p50 / p90 | 1.2 ms / 270–299 ms | 140–164 ms / 246–327 ms |
| Writable, `F_FULLFSYNC`, `fcntl` locks, hard links, swap | yes, no, no, no, no | read-only (the spike) |

What it shows:
- voidfs mounts faster and Finder shows a large folder sooner; shell listings are close but for
  `ls -l`, which pays the spike's FSKit upcalls per file.
- Sequential reads are level when the kernel reads ahead (`dd`). Finder's copy is not: voidfs
  fetched its 452 shards one at a time. The Mac client needs its own read-ahead.
- Space's random reads hit its 8 MiB blocks on disk most of the time (inferred), and a cold one
  fetches its whole 8 MiB block first (observed on 3 October); voidfs, with
  shards of about 2.4 MB and no cache on the Mac, went to the bucket for 205–210 of 300 reads.
- Space's uploader dropped `tagged.txt`'s xattrs, and an xattr write moves a file's creation date
  on its SMB volume.
- The run also found a bug in the spike's `semantics.c` (two files printed from one buffer),
  which had made a correct `RENAME_SWAP` look like data loss; it is fixed, and the spike's FAT
  finding is marked unconfirmed.

### Against SpaceFS's ratios

Step 3 is done when every row is at least as fast, relative to the bare bucket, as SpaceFS's.
Scored on the local runs, which are not SpaceFS's setup:

| Run | Rows at or ahead of SpaceFS | Edits (24) | Writes (11) | Reads (10) | Metadata (4) |
|---|--:|--:|--:|--:|--:|
| **Another setup:** bucket 12 ms away and capped at S3's bandwidth (`BENCH_BANDWIDTH=s3`), 1 October, with coalesced shard fetches and shared read-ahead (two runs) | 45 | 24 | 11 | 6 | 4 |
| **Another setup:** bucket 12 ms away and capped at S3's bandwidth (`BENCH_BANDWIDTH=s3`), 1 October, with the cache drop (two runs) | 46 | 24 | 11 | 7 | 4 |
| Bucket 12 ms away, 1 October, with coalesced shard fetches and shared read-ahead (two runs) | 30–31 | 12–13 | 7–9 | 6 | 4 |
| Bucket 12 ms away, 1 October, `main` in the same session as coalesced shard fetches (two runs) | 29 | 12 | 7 | 6 | 4 |
| Bucket 12 ms away, 1 October, with the cache drop (two runs) | 27–28 | 11–12 | 6–7 | 6 | 3–4 |
| Bucket 12 ms away, 1 October, `main` in the same session as the cache drop (two runs) | 26–28 | 10–12 | 6 | 6 | 4 |
| Bucket 12 ms away, 30 September, with multipart completion in two round trips (two runs) | 29 | 12–13 | 6–7 | 6 | 4 |
| Bucket 12 ms away, 30 September, `main` in the same session as multipart completion (two runs) | 25–27 | 9–11 | 6 | 6 | 4 |
| Bucket 12 ms away, 30 September, with each touched shard chunked once per patch (two runs) | 25–30 | 9–14 | 6 | 6 | 4 |
| Bucket 12 ms away, 30 September, `main` in the same session as patch once (two runs) | 26–27 | 10–11 | 6 | 6 | 4 |
| Bucket 12 ms away, 30 September, with the group-commit hold (two runs) | 25–26 | 9–10 | 6 | 6 | 4 |
| Bucket 12 ms away, 30 September, `main` in the same session as the hold (two runs) | 22 | 9–10 | 3–5 | 6 | 2–3 |
| Bucket 12 ms away, 29 September, with small files in the log (two runs) | 24–26 | 11 | 4–6 | 6 | 3 |
| Bucket 12 ms away, 29 September, `main` in the same session as small files (two runs) | 22–23 | 10–11 | 3 | 6 | 3 |
| Bucket 12 ms away, 29 September, with fewer round trips per write (two runs) | 20–23 | 9–11 | 3 | 6 | 2–3 |
| Bucket 12 ms away, 29 September, `main` in the same session (two runs) | 22–24 | 10–13 | 3 | 6 | 2–3 |
| Bucket 12 ms away, 28 September, with the shard cache's admission (two runs) | 21–22 | 9–12 | 2–3 | 6 | 2–3 |
| Bucket 12 ms away, 28 September, `main` with group commit, in the same session (two runs) | 16–19 | 8–10 | 3 | 3 | 2–3 |
| Bucket 12 ms away, 28 September, with group commit (two runs) | 17 | 9–10 | 2 | 3 | 2–3 |
| Bucket 12 ms away, 28 September, before group commit (two runs) | 6–7 | 0 | 1–2 | 3 | 2 |
| Bucket 12 ms away, 28 September, with checkpoints and the capability probe | 7 | 0 | 2 | 3 | 2 |
| Bucket 12 ms away, 28 September, with garbage collection | 7 | 0 | 2 | 3 | 2 |
| Bucket 12 ms away, 27 September | 6 | 0 | 1 | 3 | 2 |
| Loopback, 30 September, with multipart completion in two round trips (four runs) | 34–36 | 20–22 | 8–10 | 2 | 3 |
| Loopback, 30 September, `main` in the same session as multipart completion (four runs) | 35–36 | 21–22 | 8–9 | 2 | 3 |
| Loopback, 30 September, with each touched shard chunked once per patch (four runs) | 34–37 | 21–22 | 8–10 | 2 | 3 |
| Loopback, 30 September, `main` in the same session as patch once (four runs) | 34–37 | 20–22 | 8–10 | 2 | 3 |
| Loopback, 30 September, with the group-commit hold (four runs) | 33–35 | 19–21 | 8–10 | 2 | 3 |
| Loopback, 30 September, `main` in the same session as the hold (four runs) | 31–35 | 19–21 | 7–10 | 1–2 | 3 |
| Loopback, 29 September, with small files in the log (four runs) | 34–35 | 20–21 | 9–10 | 2 | 3 |
| Loopback, 29 September, `main` in the same session as small files (four runs) | 34–36 | 20–21 | 8–10 | 2 | 3 |
| Loopback, 29 September, with fewer round trips per write (four runs) | 34–35 | 20–21 | 8–10 | 2 | 3 |
| Loopback, 29 September, `main` in the same session (four runs) | 31–32 | 19–20 | 7 | 2 | 3 |
| Loopback, 27 September | 20 | 13 | 4 | 0 | 3 |

With coalesced shard fetches and shared read-ahead (1 October,
[bench/results/shard-fetch](../bench/results/shard-fetch/README.md)):
- **Concurrent misses of a shard make one request to the bucket,** and a GET reads up to 32 shards
  ahead, borrowing past 8 from a budget all GETs share. The eight readers of a cold 64 MiB object
  make 3.6 GETs per read instead of 24.
- **Cold, the large reads are now ahead of SpaceFS's cache-cleared figures,** as a fraction of the
  bare bucket's time, `main` → this branch, A B B A B A A B:

  | Row | Capped | Capped, no total | No cap | SpaceFS |
  |---|--:|--:|--:|--:|
  | get 32 MiB | 0.71–0.76 → 0.19–0.31 | 0.31–0.36 → 0.24–0.30 | 1.18–1.32 → 0.66–0.68 | 0.50 |
  | get 64 MiB | 0.71–0.73 → 0.18–0.24 | 0.27–0.31 → 0.14–0.15 | 1.17–1.24 → 0.55–0.56 | 0.38 |

  Without a cap the bare bucket reads 64 MiB in 134 ms, not S3's 779, so that column does not
  compare with SpaceFS's. Coalescing alone, with a window of 8, gave 0.29–0.42 for get 64 MiB with
  the cap, behind SpaceFS in two runs of four; reading further ahead put it ahead in all.
- **A fixed wider window was measured and not built:** alone, a cold read gains as much from it,
  but many reads at once gained nothing, held 17–40% more memory, and without the cap's total ran
  up to 2.6× slower. The shared budget reads as far ahead alone and as 8 each when many read.
- **Pages are now checked against their hashes,** in reads and in garbage collection's marking: a
  checkpoint segment that still parsed but was not what had been written loaded a drive in a state
  it never had.
- **Warm, with the cap,** 45 of the 49 rows are at or ahead of SpaceFS's ratio in both runs. Behind
  are the three warm large reads, as before (0.78–0.94 of SpaceFS's ratio), served from memory at
  this Mac's loopback limit (versitygw, serving the same reads from the page cache, is only 6–12%
  faster), and the range read, on SpaceFS's ratio within a tenth of a millisecond.
- Warm, at 12 ms without the cap, the geometric mean against `main` is 0.980 over the 49 rows.
- A cold 64 KiB range still fetches its whole shard (about 3× the bare bucket's time), as SpaceFS's
  S3 layer does by its docs ("Fetches only the shards the range touches"). Its Mac client's strings
  name pieces of shards of up to 1 MiB, but a cold random 4 KiB read through its mount fetches one
  whole 8 MiB block and waits for it (observed on 3 October). **Decided:** the gateway stays as it
  is, and the mount (step 5) revisits pieces; the evidence and options are in the results.

With cold reads and S3's bandwidth (1 October,
[bench/results/cold-reads](../bench/results/cold-reads/README.md)):
- **The local bucket now can have S3's bandwidth** (`BENCH_BANDWIDTH=s3`): 95 MB/s down and
  68 MB/s up per connection, 1,000 MB/s in all each way, fitted to SpaceFS's bare-bucket figures
  for the rows that move the most data (within −7% to +13%; the edits in 32 and 64 MiB files,
  not used to fit it, within −1% to +5%). Not confirmed: the total for downloads, which no bare
  row of theirs reaches, and what limited their multipart uploads. S3's time per request is not
  emulated, for either side.
- **With it, 46 of the 49 rows are at or ahead of SpaceFS's ratio**, in both runs. The large
  reads are 12–14× faster than the bare bucket, the edits in 32 and 64 MiB files 3.4–37×, the
  edits in 1 MiB files 1.1–1.2×, and put 32 and 64 MiB 1.7–1.8× (voidfs uploads a body's shards
  in parallel, the bare bucket's single PUT is held to one stream's rate). Behind: get 64 MiB
  and stream get 64 and 256 MiB, warm, at 0.79–0.91 of SpaceFS's ratio: voidfs serves 64 MiB
  from memory in 50–57 ms eight at once, SpaceFS in 45–47.
- **Cold reads** (`BENCH_COLD=1`: voidfs-server's caches dropped with `SIGUSR1` before every wave
  of reads), against SpaceFS's cache-cleared figures, as a fraction of the bare bucket's time:

  | Row | voidfs, capped | Capped, no total | No cap | SpaceFS |
  |---|--:|--:|--:|--:|
  | get 4 KiB (in a pool without `inline-data`) | 0.02 (0.96–0.98) | 0.02 | 0.02 | 1.08 |
  | get 1 MiB | 1.03–1.04 | 1.03–1.04 | 1.04–1.05 | 1.18 |
  | get 32 MiB | 0.72–0.74 | 0.32–0.39 | 1.16–1.18 | 0.50 |
  | get 64 MiB | 0.69–0.71 | 0.29–0.30 | 1.12–1.16 | 0.38 |

  The small rows are ahead. The large rows are behind with the cap because each of the eight
  readers of a cold object fetches every shard of it, 537 MB for a wave of 64 MiB reads, which
  the 1,000 MB/s total holds to about 510 ms; without the total they are ahead. A cold 64 KiB
  range takes about 3× the bare bucket's time: it fetches its whole shard. Coalescing concurrent
  fetches comes first in step 3, item 6.
- Warm and without the cap, the server's change costs nothing: over the 49 rows at 12 ms, the
  geometric mean against `main` is 0.999, and the rows that looked slower moved −3.1% to +4.1%
  in voidfs's own time when run focused again.

With multipart completion in two round trips (30 September,
[bench/results/multipart-complete](../bench/results/multipart-complete/README.md)):
- At 12 ms, completion answers in 32 ms at the median, where it took 157 ms for 8 parts and 258 ms
  for 16. In focused runs, multipart put 64 MiB × 8 MiB took 254 ms instead of 343 (−26%) and
  256 MiB × 16 MiB 1,008 instead of 1,147 (−12%), less in every pair. One binary switched
  between the two paths gave −23% and −2%: in its runs the local disk had slowed down, and eight
  uploads of 256 MiB at once waited for it.
- Relative to the bare bucket, the two rows move with its own multipart times, which varied 2×
  within the session: 0.74× and 1.50× its time (64 MiB) and 1.50× and 0.80× (256 MiB) in the full
  runs, against `main`'s 1.45× and 1.70×, and 1.58× and 1.25×. Both builds were ahead of SpaceFS's
  2.44× and 1.79× in every run, so these rows do not change the count; 29 against 25–27 is the
  edits' spread.
- On loopback, where a round trip costs little, the uploads did not move beyond the spread.
- Racing or retried completions of one upload commit once, and a completion whose client goes
  away still ends the upload. The format is unchanged.
- Over the 49 rows the geometric mean ratio against `main` is 0.967 at 12 ms and 1.015 on
  loopback. The rows that looked slower in the full runs (edits and a fan-out get, none of which
  run this code) were not, run focused again at 12 ms (−4.7% to +0.8%) and on loopback (−4.2% to
  +6.2%), where one binary switched between the two multipart paths moved them as much.

With each touched shard chunked once per patch (30 September,
[bench/results/patch-once](../bench/results/patch-once/README.md)):
- At 12 ms, in focused runs, relative to the bare bucket: patch 16 × 4 KiB in 1 MiB went from
  1.71× to 1.34× (SpaceFS 1.45×) and in 32 MiB from 0.81× to 0.69× (0.70×). Both are at or ahead
  of SpaceFS's ratio in every focused run, and were in none of `main`'s. Patch in 64 MiB stayed at
  0.51× (0.47×): its edits rarely share a shard.
- On loopback, patch in 1 MiB takes 5.1 ms, what one 4 KiB write in the file takes, from 18.2
  (4.46× the bare bucket to 1.33×).
- In the full runs at 12 ms the count of rows ahead moved within its spread (25–30, `main` 26–27
  in the same session): one branch run had a fast bare bucket in the 32 and 64 MiB rows. The
  geometric mean ratio against `main` is 0.987 over the 49 rows, and 0.967 on loopback.
- Bytes, sizes and requests to the bucket are as before. The extents, and so the ETag, are the
  same in every in-file patch tried; pages written beside each other into a zero run can come out
  with other chunk boundaries, since edit by edit they depended on the order the pages came in.

With a hold before each log entry (30 September,
[bench/results/group-commit-hold](../bench/results/group-commit-hold/README.md)):
- At 12 ms, eight to 64 at once, a small write takes one round trip instead of two. Relative to the
  bare bucket, in focused runs: put 4 KiB 2.18× to 1.20× (SpaceFS 2.01×), overwrite 4 KiB 2.19× to
  1.15× (3.05×), the fan-out puts of 4 KiB at 32 and 64 2.46× and 2.48× to 1.39× and 1.47× (2.23×
  and 2.41×). Rename 64 MiB and the folder move take 14 ms, from 27: rename at 0.116× crossed
  SpaceFS's 0.126×. All six are ahead of SpaceFS's ratio in every run.
- Over the 49 rows the geometric mean ratio against `main` is 0.932 (writes 0.830, metadata
  0.747), and the geometric mean speed-up over the bare bucket is 2.9×, above SpaceFS's 2.8×.
- An entry is held only while the clients it answered come back within 2 ms, which edits (they
  upload a shard first) do not; edits in 1 MiB files moved −3% to +6%, not the same way in each
  pair. On loopback, where an entry takes 0.3 ms, nothing is held.
- List 200 keys is 0.1 ms slower (1.1 ms, still 31–38× faster than the bare bucket), for a reason
  not found.

With small files held in the log (29 September,
[bench/results/small-content](../bench/results/small-content/README.md)):
- At 12 ms, relative to the bare bucket: fan-out put 1,000 × 4 KiB at 64 at once went from 3.05×
  to 2.28× and is ahead of SpaceFS's 2.41×; at 32, from 3.05× to 2.24× against SpaceFS's 2.23×;
  put 4 KiB from 2.37× to 2.12× against 2.01×; overwrite 4 KiB from 2.57× to 2.16× (ahead already).
  Over the 49 rows the geometric mean ratio against `main` is 0.957 (writes 0.883).
- One at a time, put and overwrite 4 KiB take 1.04× the bare bucket's time, from 2.02×: one round
  trip. Eight at once take two, because while a log entry is in flight the requests the last one
  answered wait for it and then their own; two at once already do. A diagnostic hold before an
  entry took the four rows to 1.1–1.4× and rename 64 MiB from 26 ms to 13; it is the next thing to
  try, in its own change.
- On loopback, the fan-out puts and overwrite 4 KiB are now faster than the bare bucket, and put
  4 KiB takes 1.15× its time (SpaceFS's ratio: 2.01×).
- Checkpoints of a drive of small files store them as shards in the background; over 20,000 puts
  at 12 ms, p99.9 was 41–43 ms against `main`'s 51–52, and the slowest write 44–46 against 57–64.

With fewer round trips per write (29 September,
[bench/results/write-round-trips](../bench/results/write-round-trips/README.md)):
- At 12 ms no row crossed SpaceFS's ratio. Put 64 and 32 MiB moved most, from 2.2× and 2.1× the
  bare bucket's time to 1.3×, where SpaceFS's are 1.1× and 1.2×. The count moves by a few rows
  from run to run in either build (19 rows are ahead in all four runs).
- One at a time, put 64 MiB takes 0.9× the bare bucket's time and put 32 MiB 1.05×. Eight at once,
  the CPU of the one machine the harness, the bucket, the relay and the server share is the
  limit; the real run's separate hosts will say more.
- On loopback, put 32 MiB, overwrite 1 MiB and (in three runs of four) put 64 MiB crossed:
  34–35 rows against 31–32 for `main`.
- The writes that land on a checkpoint no longer wait for it: over 20,000 puts of 4 KiB at 12 ms,
  p99.9 went from 118–135 ms to 49–56, and none took over 100 ms.

With the shard cache's admission, at 12 ms:
- 19 rows are ahead in both runs, against 15 for `main` in the same session:
  - `head`, listing, get 4 KiB and 1 MiB, the range read, and the three fan-out gets;
  - nine edits: five of the eight in 1 MiB files, delete and insert 4 KiB at the start of
    64 MiB, write 4 KiB in 64 MiB, and patch in 32 MiB;
  - overwrite 4 KiB and multipart put 64 MiB.

  Five more are ahead in one run of the two: delete 4 KiB in the middle of 1 MiB, insert 4 KiB
  at the start of 32 MiB, patch in 64 MiB, the folder move and multipart put 256 MiB.
- The geometric mean speed-up over the bare bucket went from 2.0× to 2.6× (SpaceFS's: 2.8×), and
  voidfs beats the bare bucket on 30 rows (SpaceFS does on 31).
- Group commit before it took the geometric mean from 1.0× to 2.0×: every edit, rename, folder
  move, fan-out put, and put or overwrite of up to 1 MiB took half the time or less. Large puts
  and multipart uploads, which spend their time taking in the body, did not change.
- Multipart put 256 MiB sits at SpaceFS's 0.56× of the bare bucket and crosses it from run to
  run, mostly because the bare bucket's own time varies.
- Earlier, neither content-defined checkpoints nor the probe changed speed
  ([bench/results/checkpoints](../bench/results/checkpoints/README.md),
  [bench/results/capability-probe](../bench/results/capability-probe/README.md)).

What holds back the 27–28 rows voidfs does not yet win at 12 ms
([bench/results/shard-cache](../bench/results/shard-cache/README.md)):
- **A write takes its shards, then its log entry, one after the other (8 of the 9 writes, and
  most of the 15 edits).** Fan-out put 200 × 256 KiB is within 3% of SpaceFS's ratio; put 4 KiB,
  overwrite 1 MiB and fan-out put 1,000 × 4 KiB at 64 within 13–26%; put 1 MiB and the fan-out
  put at 32 within 35–38%. Edits inside 32 and 64 MiB files, now that the shard they rewrite is
  cached, take 34–44 ms where SpaceFS's ratio needs 17–41. Step 3, item 3. Puts of 32 and 64 MiB,
  which took twice the bare bucket's time taking in the body, now take 1.3× eight at once and
  0.9–1.05× alone (29 September). Files of up to 4 KiB now need no shard
  ([RFC 0003](../rfcs/0003-small-content-in-descriptors.md), implemented), and since the next
  point they take one round trip at 8 at once and more too.
- **Group commit alternated two groups of requests** (small writes and fan-out puts at 12 ms,
  rename): while a log entry was in flight, the requests the last one answered queued behind it,
  and each waited for that entry and then its own. **Fixed** (30 September): an entry now waits
  until those requests are back, at most 2 ms, and small writes take 1.15–1.47× the bare bucket's
  time.
- **Patch rewrote a shard once per edit (patch in 1 MiB, and in 64 MiB in one run).** Item 4.
  **Fixed** (30 September): each touched shard is chunked once, and patch in 1 and 32 MiB are
  ahead in every focused run at 12 ms. Patch in 64 MiB (0.51× against 0.47×) now spends its CPU
  hashing about 16 shards one after another.
- **Large reads can't be judged locally (get 32 and 64 MiB, and streams of 64 and 256 MiB):**
  the emulated bucket adds latency but no bandwidth limit. The bare bucket's 64 MiB get takes
  110–130 ms here, against S3's 779 ms in SpaceFS's run. Get 32 MiB now takes the same time per
  byte as get 64 MiB. Edits inside 32 and 64 MiB files are understated the same way. Only the
  real run can judge these rows.
- **Rename at 8 at once took two round trips** (24–26 ms, where SpaceFS's ratio needs 15–17), for
  the same reason. **Fixed** by the same hold: 13.9 ms (30 September).

Garbage collection left four small-write rows 3.5–7% slower, for a reason not found. With group
commit they take 30–38 ms instead of 106–851, and what is left of that difference can't be told
apart from round trips ([bench/results/gc](../bench/results/gc/README.md),
[bench/results/group-commit](../bench/results/group-commit/README.md)).

## 7. Step-by-step plan

Each step lists what it delivers and when it counts as done. Later steps depend on earlier ones.

1. **Make performance measurable.**
   - Port SpaceFS's 49 scenarios exactly into `bench/`: same sizes, concurrency, warm-ups,
     rounds and p50. It must run against any endpoint and against the bare bucket.
   - Run it in CI against MinIO.
   - Run it for real in their setup: a GCP n2-standard-8 in us-east4, S3 us-east-1, the voidfs
     server in us-east-1.
   - Benchmark the two Mac apps head to head, with the spike's mount measurements (listing,
     sequential and random reads, `semantics.c`, Finder behaviour) on the same test data. One run
     goes through SpaceFS's app on a trial drive; the other through voidfs on a bucket in the same
     region.
   - **Done when:** a table in the same format as theirs is published for both the S3 layer and
     the mount.
   - **Status (2026-09-29):** the scenarios are ported and have run locally and against R2 (§6).
     CI (GitHub Actions) runs them on every push to `main` at a tenth of the operations, with
     versitygw 12 ms away as in the local runs, and keeps the results; MinIO no longer publishes
     binaries, so the benchmark doesn't use it. Since 2026-10-01 the harness also measures cold
     reads (`BENCH_COLD=1`, in the `client-host` topology too), and emulates S3's bandwidth locally
     (`BENCH_BANDWIDTH=s3`).
   - **Status (2026-10-01):** the Mac apps are compared, on screen through computer use and with
     the spike's scripts: SpaceFS 0.2.333 on its trial drive against the FSKit spike on R2, the
     same test data, two cold runs each ([results](../bench/results/mac-head-to-head/README.md),
     §6). The mount's table is published; SpaceFS publishes none for its own. Still to do: the run
     in SpaceFS's setup, for the S3 layer's table.
2. **Finish the engine** (the Phase 1 exit criteria).
   - Garbage collection first. **Done** (E8): two phases per format §12 as amended by
     [RFC 0002](../rfcs/0002-gc-safe-against-writers.md), which closes a race in draft 1 that
     could delete a shard a new commit referenced. The protocol is model-checked and the
     implementation simulated (`crates/voidfs-server/src/gc/`). It reclaims deleted drives and
     abandoned uploads; old versions in live drives wait for retention policies (E13).
   - Content-defined checkpoint segments. **Done** (E11): a segment ends where a row's key hash
     has 12 low zero bits, within 256–8,192 rows (format §8.3), so a checkpoint stores only the
     segments that changed and a fork shares its source's. A checkpoint comes every 1,000
     commits or 16 MiB of log, counted from the last one and across restarts (§8.4). Loading a
     checkpoint fetches its segments 32 at a time.
   - The bucket capability probe. **Done** (B3): at start, a server creates `voidfs.json` again
     with its own bytes, which the bucket must refuse (format §7.2). OpenDAL's capability flags
     describe its driver, not the endpoint, so only trying tells. A bucket that ignores or
     fails the check can't open a pool for writing, unless the pool was created with
     `--commit-guard external` (§7.3: one server, which checks before each write). Lifecycle
     rules that would delete or archive the pool's objects also stop a server, and versioning
     without expiry of old versions is a warning. `voidfs-server probe` reports these and more
     (presigned URLs, CORS, object lock, modification times, the bucket's clock) and writes
     nothing new. Since step 4, item 6, it also mints storage credentials and reports what they
     reach (B4). Checked against versitygw 1.8.0 and Cloudflare R2.
   - Runs on AWS S3 and MinIO, and rclone.
     - MinIO: **done**. MinIO no longer publishes binaries or images, and its repository is
       archived, so CI builds its last release (`RELEASE.2025-10-15T17-29-55Z`) from source and
       runs the conformance suite and the clients on it. versitygw stands in for it locally.
     - rclone: **done** ([`tests/interop/rclone_smoke.sh`](../tests/interop/rclone_smoke.sh)):
       copy with multipart, `check --download`, sync, server-side copy and move, `rcat`. `rclone
       purge` fails: on a versioned bucket it deletes every version, and voidfs keeps history
       (protocol §3). `rclone delete` then `rclone rmdir` removes a drive.
     - AWS S3: **done** (29 September, from this Mac over home internet to us-east-1). The
       conformance suite passes (35/35), rclone does (10/10), and the probe reads every setting
       it checks. Of the 23 small-object scenarios, 20 are at or ahead of SpaceFS's ratio to the
       bare bucket and 16 faster than it; put 4 KiB and the two fan-out puts of 4 KiB are behind
       ([results](../bench/results/aws-small-objects.md)).
   - Virtual-host addressing. **Done** (S1): with `--virtual-host-domain <domain>` (repeatable),
     `<drive>.<domain>/<key>` reaches the same drive and key as `/<drive>/<key>`, which keeps
     working. The domain itself, and any other host, stay path-style, and without the flag
     nothing changes. The signature is checked on the request as sent, and must cover `host` in
     either style, as S3 requires (`403 AccessDenied` otherwise). Locations name the path on
     the host the request used (`/<key>` rather than `/<drive>/<key>`). The conformance runner
     runs every case either way (`--virtual-host <domain>`): 35/35 both ways, and boto3, the
     Rust SDK, curl's SigV4 and all three `aws-chunked` forms work.
   - A `docker compose` file, and health checks and metrics. **Done** (O1, and O4 but for
     tracing and structured logs). The Compose file ([`deploy/compose/`](../deploy/compose/))
     runs the server's image with its pool in a volume, in a bucket of yours, or in versitygw
     beside it, which CI brings up and tests.
     - `--admin-listen <address>` serves `/healthz`, `/readyz` and `/metrics` on a port of their
       own, since on the S3 port every path is a drive. It is off unless given: nothing on it is
       authenticated, and a second default port would collide wherever several servers run.
     - `/readyz` answers 200 once the pool is open, while the bucket answers. The server's own
       requests to the bucket vouch for it when the last one succeeded within 30 seconds;
       otherwise a HEAD of `voidfs.json` checks, at most once every 5 seconds.
     - `/metrics` (Prometheus) counts requests to the S3 port and to the bucket by operation,
       with their latency; the shard and page caches' hits, misses, evictions and size; group
       commit's transactions per log entry and log-write latency; garbage collection's phase and
       last step; drives open and uptime. No label names a drive or a key.
     - The Compose file's health check is `/readyz`, from inside the container; the admin port is
       not published. CI checks the endpoints over every kind of store and in Compose.
     - Recording costs nothing the benchmark can tell from the spread between identical runs: the
       49 rows at 12 ms and twelve runs on loopback, with focused runs of the small reads and
       writes ([results](../bench/results/health-metrics/README.md)). The harness can now count
       voidfs's requests to the bucket per scenario (`BENCH_BUCKET_REQUESTS=1`): no read in the
       49 rows reaches the bucket, and a multipart upload reads its part records one after
       another when it completes.
   - **Status (2026-09-29):** done (6 of 6).
3. **Win the rows SpaceFS loses.** The work items, with the step 1 evidence and a row-by-row
   baseline, are in [step-3-performance.md](step-3-performance.md). In order of impact:
   - Group commit: one log write per batch of mutations, not per mutation (37 rows). **Done**:
     rows at or ahead of SpaceFS's ratio at 12 ms went from 7 to 17, and write p50 stays at two
     or three round trips from 1 to 64 at once.
   - A shard cache that admits new shards. **Done**: it keeps what was used last, and holds its
     own copy of each shard. Rows at or ahead of SpaceFS's ratio at 12 ms went from 16–19 to
     21–22, and the fan-out gets take about a millisecond. A disk tier (S5) comes later.
   - Fewer sequential round trips per write: a small-file path with tiny files stored inside
     their metadata (a format change, so an RFC first), pipelined ingest, and checkpoints off
     the commit path. **Done**: put 64 MiB at 12 ms takes 1.3× the bare bucket's time eight at
     once (from 2.2×) and 0.9× alone, and writes that land on a checkpoint no longer wait for it.
     Small files are held in the log ([RFC 0003](../rfcs/0003-small-content-in-descriptors.md),
     implemented): alone, a small write takes one round trip (1.04× the bare bucket at 12 ms, from
     2.02×), and fan-out put 1,000 × 4 KiB at 64 crossed SpaceFS's ratio.
   - A hold in group commit, so that concurrent writes share one round trip. **Done**: a log entry
     waits, at most 2 ms, for the requests the one before it answered; put and overwrite 4 KiB, the
     fan-out puts of 4 KiB and rename 64 MiB went from two round trips to one at 12 ms, all ahead
     of SpaceFS's ratio.
   - Patch that rewrites each touched shard once. **Done**: the edits that share a shard are
     applied to it together and it is chunked once; patch 16 × 4 KiB in 1 MiB takes 1.34× the
     bare bucket's time at 12 ms (from 1.71×; SpaceFS 1.45×), and in 32 MiB 0.69× (from 0.81×;
     SpaceFS 0.70×).
   - A multipart completion without a chain of round trips. **Done**: completion reads the
     upload's record and the parts' together, and answers once it commits, 32 ms at 12 ms instead
     of 157–258; the staging records are deleted after. Multipart put 64 MiB × 8 MiB takes 254 ms
     at 12 ms instead of 343, and 256 MiB × 16 MiB 1,008 instead of 1,147. Racing or retried
     completions of one upload now commit once.
   - Cold reads, and S3's bandwidth, measurable locally. **Done**: `SIGUSR1` empties the
     server's caches and the harness's `--cold` drops them before every wave of reads;
     `BENCH_BANDWIDTH=s3` caps the emulated bucket as S3 was in SpaceFS's run. With the cap, 46
     of the 49 rows are at or ahead of SpaceFS's ratio; cold, the small reads are ahead of their
     cache-cleared figures and the large ones behind.
   - Parallel and coalesced shard fetch for cold and large reads. **Done**: concurrent misses of a
     shard make one GET, and a GET reads up to 32 shards ahead, borrowing past 8 from a budget all
     GETs share. Cold with S3's bandwidth, get 32 and 64 MiB take 0.18–0.31× the bare bucket's time
     (SpaceFS 0.38–0.50×, `main` 0.71–0.76×). Pages are now checked against their hashes too.
     Reading pieces of shards is deferred to the mount (step 5): SpaceFS's S3 layer reads whole
     shards for a range as well.
   - **Done when:** every one of the 49 rows is at least as fast as SpaceFS's.
   - **Status (2026-10-01):** group commit (with its hold), the shard cache's admission, fewer
     round trips per write (pipelined ingest, background checkpoints, small files in the log),
     patch chunking each touched shard once, multipart completion in two round trips, cold
     reads and S3's bandwidth in the benchmark, and coalesced shard fetches with shared read-ahead
     are done. 29–31 of the 49 rows are there in the local 12 ms runs, 45–46 with S3's bandwidth
     emulated, and 34–37 on loopback; cold, the four rows SpaceFS gives cache-cleared figures for
     are all ahead with S3's bandwidth. Next: the real run (item 7.2), which also judges the three
     warm large reads.
4. **Client core, CLI and Rust SDK.** The work items, with what SpaceFS's CLI and app do, are in
   [step-4-client.md](step-4-client.md). In order:
   - The Rust SDK (A1). **Done** (1 October): `crates/voidfs-sdk`, the official AWS SDK for S3 with
     a typed call for every extension and the change feed (long poll and events), one error type,
     and the retry rule: an unguarded insert or removal is never sent twice. Its tests run against
     a server in the same process, which `voidfs-server` now offers as a library, and through a
     proxy that fails requests; the conformance runner signs with the SDK's code.
   - The CLI on the protocol (D9), whose command is `void`. **Done** (1 October): drives, drive,
     fork, history (of a file, or of every file in a folder), show, restore, version, a foreground
     upload (multipart through the AWS client for large files), and a key generator for
     `voidfs-server --key`; JSON output everywhere, errors included. Its tests run the binary
     against a server in the same process. It found three server issues, all fixed: the
     recently-deleted listing missed a key equal to its prefix; a folder restore wasn't in the
     histories of the files it changed (RFC 0004); and `lastVersionId` named the transaction's
     target, not the object
     ([step-4-client.md](step-4-client.md#item-2-the-cli-on-the-protocol)).
   - `crates/voidfs-client` (D1): the disk cache, the write journal, the upload queue (pause,
     resume, a bandwidth cap) and the change-feed client. **In progress** (2 October): the cache
     and fetcher are built (8 MiB blocks on disk and in memory, verified, with read-ahead), and
     measured against the head-to-head
     ([bench/results/client-cache](../bench/results/client-cache/README.md)); so are the write
     journal and the upload queue (ordered, coalesced, guarded publishes with the `412` rule;
     pause, resume and cancel by scope; multipart resumption; a bandwidth cap that applies at
     once), and the change-feed client with connectivity. **Done** (2 October).
   - The daemon, and the CLI's commands on it: daemon, upload, uploads, status, and the mount table
     that mount, unmount and mounts use (mounting comes with step 5). **Done** (3 October): the
     daemon on its socket (HTTP and JSON), `void daemon run|start|stop|restart|status|info` and
     `void status`; uploads through it (`void upload --detach`, `void uploads`), which survive its
     restart; the mount table and remembered mounts (`void mount|unmount|mounts`, which answer
     `NoAdapter` until step 5), and a launchd agent (`void daemon install|uninstall`).
   - Direct uploads (E10, §4.11), server and client. **Done** (4 October): the server offers them
     where the bucket binds a shard's checksum to its URL (checked at start); the SDK sends them,
     opt-in as SpaceFS's does, and the upload queue sends a file that way when the drive holds most
     of it. A 32 MiB file with 4 KiB changed went in 404 ms and 2.6 MB where a put took 2,023 ms
     and 33.5 MB, over a link of 12 ms and 20 MB/s up
     ([bench/results/direct-uploads](../bench/results/direct-uploads/README.md)).
   - Short-lived storage credentials (B4, §5.5): AWS STS, R2, MinIO. **Done** (4 October): the
     format reader is factored out of the server (`voidfs-format`); the server mints credentials
     through provider minting, scoped to what a drive's reader needs and checked at start, where
     it can (MinIO STS, AWS STS with a role, R2's Cloudflare API), and answers `501` elsewhere;
     and the client core reads shards and the drive's state straight from the bucket with them,
     through the API where it can't.
     12 ms away and 40 MB/s down, a 64 MiB file read cold in 1.9 s from the bucket, 3.2 s through
     the server ([bench/results/storage-credentials](../bench/results/storage-credentials/README.md)).
     R2's credentials (Cloudflare's API with `VOIDFS_R2_API_TOKEN` and a static parent key) are
     verified live: the scoped reader's two operations return 200, the other four are refused,
     and four conformance cases pass. AWS also passes all four after correcting the read role's
     bucket ARNs, which initially named a different bucket. Both capability scripts exit 0
     and purge four test-pool objects. R2 left zero objects and AWS preserved its two
     pre-existing objects. The [AWS guide](aws-storage-credentials.md) records both the
     initial AccessDenied result and the successful rerun.
     Presigned URLs as a fallback, and an object's shard list for the API path, would be
     protocol additions (RFC first): the user deferred both on 4 October.
   - **Status (2026-10-04):** items 1–6 of 6 done.
5. **Writable macOS drive** (Phase 2).
   - The ordered deliverables, adapter/bridge decisions and validation gates are in the
     [step 5 plan](step-5-macos.md). Its namespace, snapshot-read, local namespace-mutation,
     staged file-data, Rust daemon-session, guarded-publication and recovery slices are
     implemented, the core advertises its capabilities and sets mode and mtime, and item 1 is
     complete; the daemon's socket carries every mount-core call, and the app's agent bridges the
     sandboxed extension to it; no writable adapter exists yet.
   - The design the spike chose: the per-user agent and a thin extension.
   - Mac file semantics (xattrs, no `._` files, atomic saves) and snapshot-at-open reads.
   - A connectivity state that fails fast when offline, and read-ahead for video.
   - Reads of pieces of shards, deferred here from step 3, item 6: bounded range GETs of up to
     1 MiB, cached and coalesced apart from whole shards, so that a cold random read does not wait
     for a whole shard. SpaceFS's client doesn't: a cold random read through its mount fetches and
     waits for an 8 MiB block (observed on 3 October), so pieces would put voidfs ahead, not level.
     Verified whole shards remain the accepted path. Authenticated pieces need a later RFC;
     length-only verification is not the selected policy. Evidence and options:
     [bench/results/shard-fetch](../bench/results/shard-fetch/README.md#ranged-shard-reads-options-not-built).
   - A privileged helper that mounts into `/Volumes`, as SpaceFS does.
   - Finder Sync badges.
   - A menu-bar transfer queue (pause, resume, speed limits).
   - A notarized Developer ID build.
   - **Done when:** the Phase 2 criteria are met. A 50 GB video project and a code repo can be
     edited from two Macs, changes show within 5 s, and the app-compatibility matrix is green.
   - **Status (2026-10-08):** item 1 of 10 done; item 2 begun. The mount session has a
     persistent namespace with stable inodes, indexed equivalent-name lookup, per-directory
     refreshes, generation invalidation and complete offline directory snapshots. Read-only
     handles bind attributes and bytes to a retained version through the shared cache, preserve
     read-ahead and reject handles from earlier sessions. Moved snapshot reads refresh the
     inode's namespace path first; fallback attribute/deleted listings resolve object identity
     within a shared per-read budget of 16 listing calls and a 2-second resolution deadline.
     Budget exhaustion returns `EAGAIN`; a failed `404` key found again returns `ESTALE`.
     Slice 3 now supplies empty-file create, mkdir, unlink, merged-empty rmdir, rename and binary
     xattrs in a durable local overlay, atomically journaled with version/absence guards. Paused
     renames retain their remote base paths; pending names/maps survive feed invalidation and restart.
     Mount publishes keep their guards on conflict. Remote rename replacement is a guarded delete
     followed by an exclusive rename, while the local replacement is atomic. File writes append
     bytes before committing logical extents and inode metadata on a separate SQLite `NORMAL`
     connection; fsync and writable close flush bytes and commit metadata at `FULL`. Reads merge
     shared local ranges with each handle's bound remote base and zero-filled gaps. Shrink/regrow
     never revives discarded bytes. Frozen snapshots enter the existing guarded queue, and files
     that stay open also flush after two seconds without writes. A 256 MiB default local reserve
     refuses further staging with `ENOSPC`; configuration supplies an injectable free-space check.
     Unlinked handles retain their local bytes without publishing later changes. Publication
     adopts exact acknowledged identity, metadata and owned names atomically with journal completion.
     Newer edits survive an earlier acknowledgement; clean staging detaches for fresh opens while
     earlier handles retain their immutable local view. Guard failures retain both local and remote
     bytes and complete xattrs; successors remain blocked, including after restart or queue
     cancellation. Recovery queues what a killed session left unflushed when the next writer
     opens, knows a mount put whose reply was lost by its marker on the version right after its
     guard, and removes staging files, frozen copies and conflict snapshots that nothing records
     or reads. Flushes compact staging files whose overwritten bytes outweigh the live ones. A
     stage that lost its bytes in a power failure marks only its own file `error`. Named kill
     points in test builds cover staging, flush, publication, reconciliation, compaction and
     conflict capture, and a seeded model test runs restarts against an in-memory filesystem.
     A refresh keeps names with unpublished changes, so a removal elsewhere becomes a conflict
     rather than lost edits. The core advertises what it doesn't support (hard links, exchange,
     cloning, cross-machine locks), refused with `ENOTSUP`, and sets mode and mtime, which
     completes item 1. Lost replies to mount edits, renames and attribute changes are recognized
     exactly (RFC 0005 lets edits carry the marker), and an entry whose reply was lost is retried
     alone. Any future
     recursive removal represented by one retained parent needs a separate addressing decision
     for its children. Bounded Rust session RPCs now run over the existing daemon socket, with
     binary data, per-consumer handle ownership/read-only policy and persisted generation
     handshakes. Mounts and RPC clients share one stable-ID core and feed watcher; committed
     metadata invalidations precede observer notifications, and reconnect/gaps trigger full resync.
     The signed Swift bridge and adapters remain pending.
     The read-only transport/adapter may proceed alongside writable core slices after the
     snapshot/restart checks and the user's signed-bundle probe, which passed on 9 October: a
     Developer ID, notarized build whose sandboxed module mounts and reaches an `SMAppService`
     helper ([step 5](step-5-macos.md#signed-bundle-probe-9-october)). The accepted direction is FSKit first, SMB evaluation on gate
     failure, the Rust daemon/socket with a thin Swift XPC bridge, and verified whole shards
     until an authenticated-pieces RFC. New names use NFC; conflict resolution UI
     remains pending. Writes survive process crashes in staging, with disk flush on
     fsync/F_FULLFSYNC/close and separate cloud status.
6. **Accounts and web app** (Phase 3).
   - Passwordless email sign-in, workspaces and roles.
   - Keys minted and revoked within a minute, with drive allowlists.
   - Bucket connections, with their credentials stored encrypted.
   - A web file browser, share links, an audit log and quotas.
   - The account commands of the CLI: login, logout, whoami, workspace, use, keys.
7. **SDKs and the agent surface.**
   - TypeScript, Python and Go SDKs.
   - `operations.json`, `llms.txt` and `llms-full.txt`.
   - An MCP server (SpaceFS has none).
8. **Search, previews and video review**, the last parts of SpaceFS's Mac app.
   - A filename index across drives and the Mac.
   - Content search.
   - Previews for the Mac app, the web and share links.
   - Video review with timecoded comments.
   - **Done when:** everything SpaceFS's Mac app offers, voidfs's does too.
9. **Linux CLI and FUSE mount**, with self-update and deb, rpm and Homebrew packages.
10. **Enterprise and scale.**
    - SSO, SAML and SCIM; admin tools; encryption; retention policies.
    - Several nodes with fenced leases.
    - Regional presence: several regions, or the Cloudflare deploy target.
    - **Done when:** the global benchmark matches in all six regions.

**After parity**, where voidfs can lead: Windows, adopting an existing bucket in place, export to
plain objects, file locking, offline pinning.

## 8. Open items

- **Cloud accounts for step 1:** an AWS bucket in us-east-1 and a GCP VM in us-east4, billed to
  the maintainer. The commands and scripts are in
  [bench/README.md](../bench/README.md#the-real-run). SpaceFS ran their layer on the client VM,
  so that topology is the like-for-like one; the plan's server in us-east-1 is a second run.
- **The SpaceFS trial** runs out on 4 October ("Choose a plan to keep your files editable after the
  trial"). On its last day, everything the remaining steps needed from it that the API, the CLI,
  Finder and UI scripting could reach was observed: direct uploads, mount credentials and the format
  behind them, keys and revocation, the mount's reads, pins and FSKit, and the settings ([step 4
  §1.5–§1.9](step-4-client.md#15-direct-uploads-observed-through-the-api-3-october-02343-protocol-1)),
  and the S3 layer from this Mac ([bench/results/space-endpoint](../bench/results/space-endpoint/README.md)).
  Not observed: the web app (share links, previews, video review, audit), its launcher's search,
  and forks and second drives, which its individual plan refuses.
- **CI** ([`.github/workflows/ci.yml`](../.github/workflows/ci.yml), added 2026-09-29): tests,
  clippy and the spec's cases; the conformance suite, boto3, rclone and the admin endpoints over
  memory, local disk, versitygw and MinIO; the Compose files; and on `main`, the release GC model
  and a small benchmark. Still to add: the bucket checks the capability probe can't make without new
  objects (that create-if-absent holds when writers race, and that reads and listings see
  writes at once).
- **Tools:** rclone and Docker (with Colima) are installed on the development Mac; the AWS CLI is
  not. MinIO's Homebrew build crashes on this Mac, and MinIO no longer publishes binaries or
  images, so versitygw is the local S3 server and CI builds MinIO from source.
- **An AWS S3 bucket** in us-east-1, with a key scoped to it, exists since 2026-09-29 (made as
  in [bench/README.md](../bench/README.md#the-real-run); the key is in `.env.aws`, which git
  ignores). Step 2's runs used it; step 1's real run can too. Runs against it are asked for
  first, and `voidfs-bench/` is emptied after each.
- **Order of the next steps (agreed 2026-09-28):** the shard cache's admission (step 3, item 2),
  then the rest of step 2, then fewer sequential round trips per write (step 3, item 3).
  - Content-defined checkpoint segments came first, because they change the checkpoint writer
    that step 3, item 3 moves off the commit path. The capability probe and group commit came
    next. All three are done, and so is the rest of step 2 (2026-09-29).
  - Item 2 is done. Item 3's small-file path is a format change and needs an RFC first; with
    group commit done, it is what most of the remaining write and edit rows wait on.
